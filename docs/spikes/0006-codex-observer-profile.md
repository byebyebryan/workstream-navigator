# Spike 0006: scoped Codex observer profile

## Hypothesis

Workstream Navigator can select a dedicated Codex profile only for managed
native TUIs, preserve the user's base configuration, use native hook trust,
and accept passive lifecycle observations without making unmanaged Codex
sessions observable or trusting an invocation from an agent shell.

## Procedure and isolation

The harness creates a mode-0700 temporary root containing:

- a temporary CODEX_HOME with only a mode-0600 copy of the existing auth cache;
- a synthetic base config with hooks disabled and its disposable workspace
  already trusted;
- a separately selected `wsnav-observer` profile with hooks enabled;
- one spike-owned handler for SessionStart, UserPromptSubmit, Stop, and
  SessionEnd;
- one private runtime tmux server and one private presentation server; and
- a disposable Git repository.

The first profile-selected native TUI presents Codex's hook review gate. The
harness selects native `Trust all and continue`, exits without submitting a
prompt, and starts a second profile-selected TUI. The second process submits one
fixed harmless turn, then the harness replays forged, stale-generation, missing
authority, and 300 KB unmanaged inputs against the spike handler.

The handler drains stdin before every parse or authority decision. It records
only event kind, accepted/rejected relationships, rejection reason, and
provider-ancestry depth. It never records identifiers, prompts, transcripts,
paths, PIDs, credentials, payloads, or terminal output.

Finally, the harness launches Codex from the same temporary home without the
profile, proves no observer event is produced, and exercises fail-closed
profile installation and removal ownership checks.

## Observed contract

The live automated study passed locally with Codex CLI 0.145.0; see the
sanitized [fixture][fixture].

- the selected profile enabled hooks over a base `hooks=false` value without
  changing the base config;
- Codex's startup hook-review UI persisted trust for the unchanged hook
  definition;
- the trust-only launch created no native session history;
- a new landing screen did not emit SessionStart until its first prompt created
  the thread;
- accepted event order was SessionStart, UserPromptSubmit, then Stop, followed
  by SessionEnd on normal exit;
- provider ancestry, authority, generation, and cwd all agreed for live events;
- forged-process, stale-generation, and missing-authority invocations were
  rejected;
- a 300 KB unmanaged payload was fully drained without SIGPIPE;
- an ordinary launch without `--profile wsnav-observer` emitted no observer
  event;
- foreign profile collisions and modified profile removal failed closed, while
  exact owned removal succeeded; and
- the ordinary tmux fingerprint was unchanged and cleanup completed.

## Decision and limits

The dedicated-profile and passive-hook direction is viable for V1. Setup can
use Codex's native trust UI once, and managed launches can select the profile
without changing ordinary Codex runtime behavior.

SessionStart is not proof that a blank TUI process exists: current Codex creates
the native thread on first prompt, and only then emits the initial lifecycle
events. Workstream Navigator must therefore keep a freshly launched row in
`starting` until the binding event arrives.

This spike uses Linux `/proc` ancestry as spike-only evidence; the production
host adapter still needs a platform-appropriate process-birth and ancestry
implementation. It does not override an administrator or project policy that
disables hooks. Such a policy must produce the designed `disabled` integration
state, not a weakened authority check.

The isolated home intentionally excludes unrelated user and project hooks.
Production uses the user's normal configuration, where all active hook sources
continue to coexist. Workstream Navigator neither disables nor assumes
responsibility for those integrations.

[fixture]: ../../spikes/fixtures/codex-observer-profile.json
