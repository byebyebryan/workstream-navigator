# Spike 0011: Codex native `/new` rebinding

## Hypothesis

Within one managed, private Codex TUI, native `/new` creates a distinct
thread without replacing the Codex process. If its `SessionStart` event is
observed as `source=new` by the direct-parent hook authority established in
[Spike 0010](0010-codex-hook-ancestry-authority.md), it can be considered for
the same bounded App Server corroboration and idle-or-attention binding gate as
native `/clear`.

This is an evidence study only. It does not change the V1 state machine,
roadmap, or the current fail-closed treatment of `/new`.

## Procedure and isolation

The maintained ancestry harness has a separately selected transition mode:

~~~console
spikes/codex-hook-ancestry-authority.py --transition new \
  --result spikes/fixtures/codex-native-new-rebinding.json
~~~

It creates a mode-`0700` temporary root with a disposable Codex home, two
empty Git repositories, generated spike-owned hooks, and one private tmux
server per native TUI. It copies only a mode-`0600` auth cache. It drives the
native hook-review prompt, proves concurrent Runtime identity, then attempts
two `/new` transitions in one idle TUI and sends one harmless fixed response
prompt in each destination. A missing first transition stops the study before
the second attempt or the unrelated forgery checks.

The hook drains its stdin and retains only accepted/rejected relationships,
a fixed source category, direct-parent depth, and a temporary one-way session
digest used to establish that the native thread changed. The sanitized fixture
also has aggregate counts for those fixed source categories, so an unexpected
source can be distinguished from no lifecycle event without retaining a raw
payload. The temporary root is removed before the fixture is written. It never
retains prompts, responses, terminal captures, native session IDs, paths,
process IDs, credentials, or raw hook payloads.

A passing run also rejects an external hook call, a Codex agent-tool-shell
call, and a stale Runtime record. It checks that the Codex PID birth token
stays constant through both transitions, the ordinary tmux server fingerprint
is unchanged, and all owned state is cleaned up.

## Observed result

The sanitized [fixture] is **falsified** on Codex CLI `0.146.0`. It contains
three accepted `SessionStart(source=startup)` observations from the disposable
launches, and zero `new` or unrecognized source observations. The first native
`/new` therefore did not provide any `SessionStart` evidence before its
destination turn settled.

This rules out extending the existing `SessionStart` changed-binding exception
to `/new`: there is no native hook claim to authenticate or corroborate. WSNav
must retain the old binding and fail closed after `/new` unless a separately
validated, equally strict signal is found. The result does not imply that
Codex's native `/new` workflow itself failed; only that it is currently
invisible to the passive lifecycle authority.

## Pass gate

The study passes only if both `/new` transitions yield an accepted,
distinct `SessionStart(source=new)` for the same live provider process, while
all forged/stale cases remain rejected. It is falsified if Codex emits another
source, no distinct session start, or restarts the provider process. A blocked
result means the isolated native study could not be safely completed and does
not widen production behavior.

Even a pass leaves the production gate intact until a later approved slice
adds narrowly tested `new` handling: exact active Runtime identity, only
idle-or-attention replacement, bounded `thread/read(includeTurns=false)` ID
corroboration, transactional predecessor metadata, and fail-closed behavior
for every other source or race.

[fixture]: ../../../spikes/fixtures/codex-native-new-rebinding.json
