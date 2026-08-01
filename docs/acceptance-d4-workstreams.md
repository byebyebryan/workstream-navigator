# D4 Workstream and Fork Acceptance

Date: 2026-07-30

Status: superseded for filesystem behavior — retained provider-fork evidence

> The 2026-08-01 project-root-only correction retires WSNav-managed branches
> and worktrees. This record remains evidence for exact settled-turn provider
> forks, native result preservation, and private tmux isolation only. Current
> D4 acceptance requires source and destination to retain the same registered
> project root and forbids WSNav Git mutation.

## Automated contract evidence

- The disposable harness creates an external source checkout and uses an
  isolated state root, Codex home, fake provider, and private tmux sockets.
  It never installs hooks in ordinary Codex state or uses the default tmux
  server.
- The source has one completed turn followed by a running turn. The captured
  provider request is required to name the completed turn only.
- Under the current contract, source and destination retain the registered
  project root; no WSNav-created filesystem is permitted. The source project
  stays intact.
- Exact destination resume, request idempotency, lost-response reconciliation,
  and stale/ambiguous recovery paths have parser, state, action, and transport
  coverage. Retired branch/worktree ownership checks are not current evidence.

## Recorded native-Codex acceptance

- The operator used the existing explicitly trusted observer integration. The
  normal Codex home, profile, authentication, and configuration were retained;
  Workstream Navigator did not alter any normal Codex configuration.
- A completed source turn became a bound native conversation with visible
  unseen-result attention. An explicit fork created a distinct native provider
  thread and resumed it in a directly interactive destination TUI. The source
  result remained untouched. The original run's separate managed-checkout
  observation is retired and not a current requirement.
- The destination completed an independent native turn. Its resulting tip was
  distinct from the source tip while both Workstreams retained their own
  attention state.
- A second fork was issued while the source was durably `Working`. Its
  committed operation contained one pre-existing settled-turn boundary and an
  exact-once provider-attempt marker. The source continued without interruption
  and later advanced to a newer settled tip, proving that the second destination
  was created from the earlier settled prefix rather than the running turn.
- Every managed runtime was parked at completion; no provider history was
  deleted. The original separate-checkout retention observation is historical
  only.
- The ordinary tmux fingerprint was unchanged before the run, after both forks,
  and after parking all managed runtimes. All provider sessions ran only on
  private WSNav tmux sockets.
- An optional unavailable MCP emitted a normal native warning during startup,
  but did not prevent the corroborated lifecycle binding or either fork. WSNav
  did not change the user's MCP configuration.

## Retention and privacy

The operator chose to reuse an existing observer integration and normal host
state. The resulting parked native threads remain intentionally available for
inspection and exact resume. This is not a full-uninstall or clean-host residue
claim; that belongs to D5.

The recorded fixture contains only boolean assertions and the provider
contract fingerprint. It excludes provider identifiers, prompts, responses,
terminal capture, paths, process IDs, credentials, and raw hook or App Server
payloads.

See the [D4 sanitized fixture](../spikes/fixtures/d4-local-codex-workstream-fork.json).
