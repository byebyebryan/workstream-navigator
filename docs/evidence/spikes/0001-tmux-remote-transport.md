# Spike 0001: tmux remote-session transport

## Hypothesis

Workstream Navigator can keep a provider's native terminal surface interactive
while the provider process persists remotely behind a dedicated tmux server.
The minimal path is a dedicated local tmux server, SSH with a TTY, and a
dedicated remote tmux server. No remote daemon or session protocol is needed to
prove the transport layer.

## Boundaries

The harness creates unique local and remote socket paths. It does not use the
caller's `TMUX` value or default server, touch provider configuration, create a
worktree, or launch Codex. It fingerprints ordinary local and remote tmux
server state before and after the study without recording its contents.

The remote payload is a fixed marker-based shell endpoint. It accepts two
fixed ping messages and a color probe; it receives no prompt, repository path,
credential, or provider transcript.

## Passing transport assertions

- The local and remote test servers use dedicated sockets.
- The remote host differs from the local host.
- Input crosses local tmux, SSH, and remote tmux and receives a reply.
- The remote terminal reports at least 256 colors.
- A local outer-pane resize updates the remote inner tmux window.
- The remote pane process survives local detach and is unchanged after a new
  local attachment reconnects.
- Mouse support is enabled on both dedicated tmux servers.
- Ordinary tmux state has the same pre/post fingerprint.
- Cleanup removes the disposable sockets and temporary directories.

## Limits and decision rule

A pass proves only the tmux/SSH transport substrate. It does not prove Codex
authentication, provider rendering details, full mouse interaction, image
rendering, provider status detection, workstream discovery, or a navigator UI.

The decisive provider follow-up uses the same harness shape with a reachable
authorized host that has Codex. It must confirm a real Codex TUI remains
interactive through detach/reconnect and preserves the visible result tip.

- If transport passes and the native Codex follow-up passes, V1 may use tmux
  plus SSH as its foundational persistence and remote-attachment substrate.
- If transport passes but Codex rendering or input fails, retain tmux for
  persistence but evaluate a terminal-emulation boundary or a different
  presentation layer.
- If transport fails, do not proceed as though a thin tmux/SSH design is
  validated; reassess the product boundary before building a remote protocol.

## Observed result

The sanitized fixture is [tmux-remote-transport.json][fixture]. The transport
study passed against a reachable authorized non-provider host:

- Fixed input crossed the nested path in both the initial attachment and the
  reattachment after the local server was destroyed.
- The remote pane process was unchanged before detach, after detach, and after
  reattachment.
- The remote inner terminal reported 256 colors, and a `101x31` local pane
  resize reached the remote tmux window unchanged.
- Mouse support was enabled on both disposable servers. Actual provider mouse
  interaction remains untested.
- The fingerprints of ordinary local and remote tmux state were unchanged, and
  cleanup removed all disposable socket directories.

An early harness attempt also established an important layout constraint: a
tmux status line consumes one terminal row, so it turns an outer `101x31` pane
into an inner `101x30` terminal. The accepted study disables tmux status bars
on the dedicated servers. Workstream Navigator's own navigation and status
must therefore live in its pane layout, not in a global tmux status line.

Native Codex validation was **blocked**, not falsified, for this transport-only
run: its host did not have Codex. That limited provider result is superseded by
[Spike 0002][spike-0002], which ran the native attach/detach/reconnect gate on
an authorized, reachable Codex host. The terminal-substrate decision combines
that result with local Spike 0005 and the historical Python Phase 7F terminal
trial; no separate manual terminal spike is required.

[fixture]: ../../../spikes/fixtures/tmux-remote-transport.json
[spike-0002]: 0002-codex-native-tui.md
