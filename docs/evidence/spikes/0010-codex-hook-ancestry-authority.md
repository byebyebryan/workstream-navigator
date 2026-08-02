# Spike 0010: Codex hook ancestry authority

## Hypothesis

After Codex sanitizes the managed launch environment, a passive command hook
can still be bound to exactly one private Runtime without accepting an
invocation from an agent tool shell. The candidate uses an immutable command
argument to locate private Runtime records, then requires the hook's **direct
parent** to have the recorded Linux PID, process-birth value, and working
directory.

## Procedure and isolation

The automated local study used a mode-`0700` temporary root containing a
temporary Codex home, two empty Git repositories, generated user-level
`hooks.json`, and one private tmux server per native Codex TUI. The temporary
home received only a mode-`0600` copy of the existing Codex auth cache. The
normal Codex home, configuration, hooks, history, repositories, and tmux
server were not changed.

The first disposable TUI completed Codex's native hook-review flow. Two new
private TUIs then ran concurrently. Their generated hook command contained the
temporary record-root argument; the Codex launch additionally contained a
sentinel environment variable which the hook was not allowed to require.

The handler drained stdin and recorded only event kind, acceptance relation,
known start source, direct-parent depth, and a temporary boolean that a native
session changed. It never retained provider IDs, prompts, responses, terminal
captures, paths, process IDs, credentials, or raw payloads. A one-way session
digest was temporary evidence for the `/clear` transition and was removed with
the root.

The test exercised two ordinary turns, two consecutive native `/clear`
transitions in one running TUI, an external direct fake hook, a fake hook
launched by Codex's own agent tool shell, and a live Runtime whose record was
removed. Raw metadata and all disposable sockets, processes, repositories,
and credentials were removed before the fixture was emitted.

## Observed contract

The sanitized [fixture][fixture] passed on Codex CLI `0.146.0`:

- Codex did not retain the launch sentinel in either provider environment.
- Both concurrent native TUIs independently produced accepted
  `SessionStart`, `UserPromptSubmit`, and `Stop` evidence.
- Each accepted native hook was a direct child of the recorded provider
  process, with exactly one matching PID, birth, and cwd record.
- Two native `/clear` actions each produced an accepted, distinct
  `SessionStart(source=clear)` while the managed Codex process birth remained
  unchanged.
- An external process, an agent-created command shell, and a missing/stale
  Runtime record were rejected. The agent-shell case is why this candidate
  deliberately accepts direct parentage only; allowing a generic one-hop shell
  wrapper would weaken the boundary.
- The ordinary tmux fingerprint was unchanged and cleanup completed.

## Decision and limits

This **replaces the falsified launch-environment transport as a viable
candidate**, not the observer implementation itself. A generated observer
declaration can pass its canonical host state-root as an immutable command
argument and look up only private Runtime records keyed by process PID, birth,
and cwd. It must require direct Codex parentage, exact one-record matching, bounded
stdin draining, and fail closed when the observed provider version/process
topology differs.

The result is Linux- and Codex-`0.146.0`-specific. It does not authorize a
fallback to shell-wrapper ancestry, an unbounded global observer, transcript
storage, or provider-pane management traffic. It also does not by itself prove
the production profile declaration, transactional binding update, App Server
corroboration, remote transport, or every future Codex release. Those remain
separate implementation and acceptance gates.

[fixture]: ../../../spikes/fixtures/codex-hook-ancestry-authority.json
