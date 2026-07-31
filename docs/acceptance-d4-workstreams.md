# D4 Workstream and Fork Acceptance

Date: 2026-07-30

Status: pass — disposable coverage and bounded native-Codex acceptance

## Automated contract evidence

- The disposable harness creates an external source checkout and uses an
  isolated state root, Codex home, fake provider, and private tmux sockets.
  It never installs hooks in ordinary Codex state or uses the default tmux
  server.
- The source has one completed turn followed by a running turn. The captured
  provider request is required to name the completed turn only.
- The destination checkout is created from the project's recorded base commit;
  source-only uncommitted content is absent from it. The source checkout stays
  intact.
- Managed branch/worktree ownership, provisional name behavior, exact
  destination resume, request idempotency, lost-response reconciliation, and
  stale/ambiguous recovery paths have parser, state, action, and transport
  coverage.

## Recorded native-Codex acceptance

- The operator used the existing explicitly trusted observer integration. The
  normal Codex home, profile, authentication, and configuration were retained;
  Workstream Navigator did not alter any normal Codex configuration.
- A completed source turn became a bound native conversation with visible
  unseen-result attention. An explicit fork created a distinct native provider
  thread and distinct managed checkout, then resumed it in a directly
  interactive destination TUI. The source result remained untouched.
- The destination completed an independent native turn. Its resulting tip was
  distinct from the source tip while both Workstreams retained their own
  attention state.
- A second fork was issued while the source was durably `Working`. Its
  committed operation contained one pre-existing settled-turn boundary and an
  exact-once provider-attempt marker. The source continued without interruption
  and later advanced to a newer settled tip, proving that the second destination
  was created from the earlier settled prefix rather than the running turn.
- Both forked checkouts were clean and at the recorded project base. Every
  managed runtime was parked at completion; no provider history or checkout was
  deleted.
- The ordinary tmux fingerprint was unchanged before the run, after both forks,
  and after parking all managed runtimes. All provider sessions ran only on
  private WSNav tmux sockets.
- An optional unavailable MCP emitted a normal native warning during startup,
  but did not prevent the corroborated lifecycle binding or either fork. WSNav
  did not change the user's MCP configuration.

## Retention and privacy

The operator chose to reuse an existing observer integration and normal host
state. The resulting clean managed Workstreams and their parked native threads
remain intentionally available for inspection and exact resume: V1 has no
destructive managed-worktree retirement action. This is not a full-uninstall
or clean-host residue claim; that belongs to D5.

The recorded fixture contains only boolean assertions and the provider
contract fingerprint. It excludes provider identifiers, prompts, responses,
terminal capture, paths, process IDs, credentials, and raw hook or App Server
payloads.

See the [D4 sanitized fixture](../spikes/fixtures/d4-local-codex-workstream-fork.json).
