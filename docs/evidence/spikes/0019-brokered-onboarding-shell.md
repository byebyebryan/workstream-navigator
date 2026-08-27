# Spike 0019: brokered provisional-shell onboarding

## Hypothesis / question

Can Workstream Navigator replace the Projects onboarding UI with a provisional
private shell in which the operator types an ordinary `codex ...` or
`opencode ...` command, while a broker establishes WSNav authority before the
native provider starts and the provider retains native interactive-terminal
ownership?

The study is deliberately about launch topology. It does not authorize
adopting an arbitrary existing process or treat process detection alone as a
managed-session binding.

## Procedure and isolation

The deterministic synthetic harness is
[`spikes/brokered-onboarding-shell.py`](../../../spikes/brokered-onboarding-shell.py),
with its sanitized [fixture][fixture]. It runs the fixed command
`python3 spikes/brokered-onboarding-shell.py --result spikes/fixtures/brokered-onboarding-shell.json`.
The harness creates one mode-0700 temporary root, a private tmux configuration
and socket per case, fixed fake provider/broker scripts, bounded subprocess and
polling waits, and `finally` cleanup. The fake provider records topology only
while the case is running: PID, parent PID, process-group leadership, session,
birth equality, fixed argument preservation, and a bounded output marker. The
result retains only booleans, enums, and the tmux version. Temporary paths,
process identities, commands, pane output, and all other raw records are
discarded.

The four cases are a PATH shim, a shell function that `exec`s a broker, a zsh
`preexec` hook, and a durable shell/supervisor with the provider as a
foreground child. The harness also issues `command codex`, an absolute
provider path, and a nested provider script. These bypasses are observed only
to prove that they bypass the controlled function; they are never adopted.
The ordinary tmux server is read only for a before/after fingerprint.

The two live revalidation records were run separately with their existing
bounded, isolated scripts. Codex used the established disposable mode-0600
auth-cache copy mechanism and did not modify the normal provider home,
history, configuration, or tmux server:

- `spikes/codex-hook-ancestry-authority.py --transition clear --result <temp>/codex.json`
  passed on Codex `0.150.0`. Startup SessionStart count was 2 and clear
  SessionStart count was 2; direct-parent, forgery, and cleanup assertions
  passed.
- `spikes/opencode-fresh-session.py --confirm-live-opencode --result <temp>/opencode.json`
  was revalidated on installed OpenCode `1.18.23`; [Spike
  0020](0020-opencode-1.18.23-revalidation.md) records all assertions passing.
  The historical `1.18.11` study pin and its source/fixture remain unchanged.

## Synthetic result

The fixture passed on tmux `3.7c`.

| Candidate | Fixed args | Broker invoked | Pane PID = provider | Birth equal | Provider PGID leader | Shell survives provider exit | Output survives in one tmux layer |
| --- | --- | --- | --- | --- | --- | --- | --- |
| PATH shim | yes | no | no | no | yes | yes | yes |
| Controlled shell function + `exec` | yes | yes | yes | yes | yes | no | no |
| `preexec` hook | no | yes | yes | yes | yes | no | no |
| Durable shell/supervisor | yes | no | no | no | yes | yes | yes |

All private cases cleaned up, used only private tmux servers, and left the
ordinary tmux fingerprint unchanged. The three bypass forms preserved the
fixed arguments and bypassed the function, so their required outcome is
`unmanaged_fail_closed_required`. The preexec case intentionally records
argument corruption rather than attempting an unsafe adoption.

The output column is only the fake provider's output in this one tmux layer.
It does **not** prove the current two-layer WSNav completed-output retention
contract for the `exec` candidate. That existing production/presentation
invariant remains an implementation and operator-acceptance gate.

## Hard falsifications

- The PATH shim changes the current Runtime identity: the pane PID/birth is a
  wrapper process rather than the provider. It cannot satisfy the current
  direct-parent and exact process-identity authority boundary.
- The tested preexec interception corrupts the ordinary command arguments.
  This hook shape is not a safe launch contract; another hook implementation
  would require separate evidence.
- The durable supervisor preserves a shell and completed output, but the
  provider is not the pane process and no broker is invoked. It is incompatible
  with the current direct-launch Runtime authority without a broader Runtime
  design change.
- `command codex`, absolute paths, aliases/functions outside the controlled
  shell, nested shells, and scripts can bypass a shell function. They must be
  unmanaged and fail closed, not inferred into WSNav ownership.

## Narrow consequence

Controlled shell-function `exec` is the only candidate compatible with the
current Runtime identity contract. A provider-specific function can pass the
ordinary arguments to a broker; the broker must reserve the private Runtime
authority and launch barrier before `exec` replaces the shell with the native
provider. The resulting provider must remain the pane process and process-group
leader so the existing observer authority can bind to exact process identity.

This is an implementation-planning direction, not design approval. The broker
still needs explicit rollback on cancellation or provider launch failure,
unique reservation for concurrent onboarding shells, and fail-closed handling
for every bypass and ambiguous observation. Navigator management traffic must
remain outside the provider pane.

## Boundaries still requiring implementation acceptance

- A blank Codex TUI does not expose an exact native session identity at shell
  launch; existing evidence confirms the initial SessionStart only when the
  first prompt creates the thread. A new row must remain `starting` until that
  provider event. Exact resume binding remains an adapter acceptance case.
- OpenCode onboarding requires its blank-session/endpoint setup, guardian or
  sidecar, and generation authority before native launch. Spike 0020
  revalidated that historical fresh-session contract on `1.18.23`, but this
  does not validate the brokered shell.
- The harness launches at one synthetic root. Exact shell cwd versus canonical
  Git root, especially a launch from a repository subdirectory, remains an
  unresolved boundary; the broker must not silently substitute one for the
  other.
- Provider exit and Ctrl-C cleanup are synthetic observations. Launch failure,
  cancellation before `exec`, two simultaneous onboarding shells for one
  repository, and native TUI behavior remain unproven.
- The harness does not prove real Codex/OpenCode native-terminal fidelity or
  the outer presentation layer's completed-output retention.

## Deterministic implementation seams

The next implementation slice should inject: broker reservation and rollback,
provider launch success/failure, cancellation timing, exact cwd/root inputs,
and a fake provider that reports PID/birth/PGID and exits deterministically.
Tests should use one private Runtime tmux server per case plus a disposable
outer presentation layer, race two reservations at one repository, and keep
the bypass table and no-provider-pane-traffic assertion explicit.

## Status

**Needs implementation planning.**

[fixture]: ../../../spikes/fixtures/brokered-onboarding-shell.json
