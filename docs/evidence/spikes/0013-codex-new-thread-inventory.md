# Spike 0013: Codex `/new` thread inventory

## Hypothesis

Native `/new` starts a distinct Codex chat/thread even though it does not emit
a changed passive hook identity. In a disposable Codex home where exactly one
native TUI owns a unique temporary workspace, bounded App Server inventory for
that workspace should grow by exactly one after `/new` and its first prompt.

This study answers whether the provider created B. It intentionally does not
claim that a production `thread/list` snapshot can identify the active TUI when
multiple Runtimes share a Project root.

## Observed result

The sanitized [fixture] **passed** on Codex CLI `0.146.0`. Each of two native
`/new` actions grew the target temporary workspace’s App Server inventory by
one distinct thread, while the same Codex process, ordinary tmux fingerprint,
and forged/stale authority checks remained valid. The fixture records aggregate
growth `2`, not thread IDs or any provider content.

This confirms that `/new` creates B rather than reusing A. It does not change
the conclusion of [Spike 0011](0011-codex-native-new-rebinding.md) or [Spike
0012](0012-codex-new-prompt-session-rotation.md): the passive observer still
receives no exact B identity, so WSNav cannot safely select B from the
inventory in a real concurrent Project.

## Procedure and isolation

The ancestry harness runs a dedicated `new-inventory` variation:

~~~console
spikes/codex-hook-ancestry-authority.py --transition new-inventory \
  --result spikes/fixtures/codex-new-thread-inventory.json
~~~

The harness creates a mode-`0700` temporary root with a private Codex home,
two empty repositories, and one private tmux server per TUI. The target TUI is
the only live one at its temporary workspace. After its initial harmless turn,
the study opens a short-lived App Server, performs one bounded
`thread/list(useStateDbOnly=true)` for that workspace, runs native `/new` and
a harmless destination prompt, waits for the destination to settle, and repeats
the inventory. It performs this twice.

Raw App Server responses and thread IDs exist only in process memory. The
comparison uses one-way digests temporarily and commits only aggregate growth
counts. The temporary home, repositories, hooks, App Server processes, tmux
servers, and credentials are removed before the fixture is written.

## Pass gate

Each native `/new` must grow the target workspace’s thread inventory by exactly
one distinct ID while the Codex PID birth token remains unchanged, the ordinary
tmux fingerprint is unchanged, and all owned state is cleaned up. Any missing,
extra, or ambiguous inventory difference falsifies creation in this provider
configuration. Hook event ordering is deliberately out of scope for this direct
App Server creation study.

A pass establishes only that `/new` creates B. It does not authorize WSNav to
choose the most recent thread in a real Project: another concurrent TUI can
legitimately create the same inventory delta. The existing direct-parent hook
authority still lacks a B identity and `/new` remains fail-closed.

[fixture]: ../../../spikes/fixtures/codex-new-thread-inventory.json
