# Spike 0002: native Codex TUI over remote tmux

## Hypothesis

After the transport layer passes, a real native Codex TUI can run remotely in a
dedicated tmux server while a disposable local tmux server supplies the visible
interactive terminal surface. A local detach must not restart Codex or change
its completed visible result tip; a later local attachment must recover the
same surface.

## Procedure and isolation

The study ran `tmux-remote-transport.sh --native-codex` against an authorized
remote host. It first re-ran the fixed-message transport assertions, then
created an empty temporary remote workspace and a temporary `CODEX_HOME` below
the remote spike directory.

The temporary home received only a mode-`0600` copy of the remote account's
existing file-based Codex credential cache. It did not receive the ordinary
Codex configuration, hooks, sessions, skills, logs, or provider state. Codex
therefore showed its first-use workspace-trust prompt. The harness accepted
only that expected prompt for its own empty workspace, waited for the native
screen to stabilize, submitted a harmless fixed request through the native
composer, and sent the submit key as a separate terminal event.

After the completed result became visible, the harness captured the visible
provider screen privately, destroyed the disposable local tmux server and SSH
attachment, verified that the remote Codex pane process was unchanged, then
created a new local server and attached it to the same remote session. It
compared the reattached visible provider screen byte-for-byte with the captured
result tip. It sent no post-result provider input.

All temporary directories, the copied credential cache, both private tmux
servers, and private raw captures were removed. The committed fixture contains
only sanitized booleans, version, timing, and cleanup status.

## Observed contract

The automated study passed for Codex CLI `0.145.0`; see the sanitized
[fixture][fixture]. It established:

- native keyboard text and submit delivery through local tmux, SSH, and remote
  tmux;
- a stable remote Codex pane process across local detach and reattach;
- a byte-identical visible result tip after reattachment, before any later user
  action;
- 256-color and resize propagation plus mouse capability configuration on the
  disposable tmux servers; and
- unchanged fingerprints for both hosts' ordinary tmux state and complete
  cleanup.

An early diagnostic showed that Codex's first-run trust screen must complete
before a composer probe is accepted, and that its text and submit key need to
arrive as separate terminal events. The accepted fixture comes from the final
fully automated run; the assisted diagnostic is not evidence.

## Limits and decision impact

This proves the native Codex presentation, input, persistence, reconnect, and
result-tip-preservation contract for this installed CLI version. It does not
independently exercise actual mouse interaction inside Codex, image rendering,
workstream discovery, thread lifecycle operations, or a navigator UI. The
terminal-substrate decision now combines this result with Spike 0005 and the
historical [Python Phase 7F terminal trial][phase-7f]; no additional manual
terminal spike is a pre-implementation gate.

The transport-plus-provider gates are now sufficient to retain a dedicated
tmux plus SSH design as Workstream Navigator's V1 persistence and remote
attachment substrate. They do not authorize a production implementation or a
remote daemon.

[fixture]: ../../spikes/fixtures/codex-native-tui.json
[phase-7f]: https://github.com/byebyebryan/agent-switchboard-python-reference/blob/main/docs/phase-7f-acceptance.md
