# Spike 0012: Codex `/new` first-prompt session rotation

## Hypothesis

Native `/new` does not emit the `SessionStart` source required by the existing
same-TUI `/clear` rule. Its first destination prompt may nevertheless emit an
authenticated `UserPromptSubmit` with a new current `session_id`. If the
direct-parent authority can prove that rotation without a new `SessionStart`,
the event can be considered for the existing bounded App Server corroboration
and idle-or-attention binding gate.

This is a candidate signal only. It does not change the V1 state machine,
roadmap, or current fail-closed behavior.

## Procedure and isolation

The maintained ancestry harness runs a separate `new-prompt` transition mode:

~~~console
spikes/codex-hook-ancestry-authority.py --transition new-prompt \
  --result spikes/fixtures/codex-new-prompt-session-rotation.json
~~~

It creates the same mode-`0700` temporary root, disposable Codex home, empty
repositories, spike-owned hooks, and private tmux servers as Spike 0010. After
the ordinary initial turns bind each exact Runtime, it performs two native
`/new` actions in one idle TUI and submits one harmless fixed prompt in each
destination. A missing first rotation stops the study before the second attempt
or the unrelated forgery checks.

The hook retains no IDs. It computes a temporary one-way session digest only
for accepted `SessionStart` and `UserPromptSubmit` evidence from an exact
direct parent. On a changed prompt session it passes that in-memory ID to one
short-lived `codex app-server --listen stdio://` process for exactly one
`thread/read(includeTurns=false)` request, then closes and reaps it. The
emitted fixture has aggregate fixed-source counts and rotation/correlation
counts, but no prompts, responses, captures, paths, process IDs, credentials,
raw payloads, or identifiers.

Each destination must produce one changed `UserPromptSubmit` session ID, with
no additional `SessionStart`. A passing run then proves the same external,
agent-shell, and stale-record forgeries remain rejected; it also proves a
stable Codex process birth, unchanged ordinary tmux fingerprint, and complete
cleanup.

## Observed result

The sanitized [fixture] is **falsified** on Codex CLI `0.146.0`. It contains
three accepted `SessionStart(source=startup)` observations and zero changed
`UserPromptSubmit` session IDs or App Server correlations. The first native
`/new` destination prompt therefore retained the original hook `session_id`.

Together with [Spike 0011](0011-codex-native-new-rebinding.md), this eliminates
both documented passive hook candidates: `/new` produces neither a changed
`SessionStart` nor a changed first-prompt session identifier. The direct-parent
authority remains sound, but it has no exact native-thread claim to authorize a
tip replacement. WSNav must continue to retain A and fail closed after `/new`.

Ordering `thread/list` results, reading transcript files, inspecting the
terminal, or watching user input would not repair that missing authority: each
is ambiguous across concurrent native TUIs or violates the privacy/native-UI
boundary. A future solution needs a separately documented provider signal that
binds the active native TUI to its new thread.

## Pass gate

The candidate passes only when both native `/new` destination prompts produce
distinct, direct-parent-authenticated `UserPromptSubmit` session rotations,
whose exact IDs also pass the bounded App Server read, without a new
`SessionStart`. A missing rotation/correlation, any unexpected session-start
event, a process restart, or weakened forged/stale rejection falsifies it.

A future approved delivery slice would still need to add the narrow runtime
transition: only `idle` or `attention` may accept a changed prompt session,
the `thread/read(includeTurns=false)` response must exactly equal that session,
the update must preserve predecessor/result-attention metadata atomically, and
the new binding becomes `working` for that exact submitted turn. A blank
`/new` remains intentionally invisible until the user submits its first prompt.

[fixture]: ../../../spikes/fixtures/codex-new-prompt-session-rotation.json
