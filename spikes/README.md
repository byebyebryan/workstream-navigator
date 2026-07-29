# Spikes

Spikes are disposable decision studies. They do not establish a production
contract, install a service, modify an ordinary tmux server, or launch a coding
agent without an explicit operator action.

## `tmux-remote-transport.sh`

This study exercises the proposed transport path:

```text
dedicated local tmux socket -> SSH -> dedicated remote tmux socket
```

It creates a disposable remote shell endpoint, proves fixed-message input,
256-color capability, resize propagation, remote process survival after local
detach, and reconnection to that same process. It enables tmux mouse support on
the disposable servers only; actual provider mouse behavior remains a manual
acceptance check.

Run it only against a host you are authorized to access:

```console
spikes/tmux-remote-transport.sh --host example-host --result /tmp/wsnav-spike.json

# Include the real, isolated Codex TUI gate.
spikes/tmux-remote-transport.sh --host example-host --native-codex \
  --result /tmp/wsnav-codex-spike.json
```

The optional result is sanitized JSON written with mode `0600`. It contains no
host names, paths, process IDs, prompts, transcripts, or credentials. The
script removes both dedicated tmux servers and temporary directories before it
writes a result.

`--native-codex` creates an empty temporary workspace and `CODEX_HOME` on the
remote host. It copies only the existing remote `auth.json` into that private
home, accepts Codex's trust prompt for that empty workspace, submits one
harmless native-TUI probe, then destroys the local attachment and verifies the
same remote process and visible result tip after reconnecting. No global Codex
configuration, hooks, sessions, or ordinary tmux server are used.

`--debug-output` is intentionally not part of normal operation. It writes a
single raw pane capture at the supplied path with mode `0600` for diagnosis;
delete it after inspection. Normal runs retain no raw terminal content.

`pass` without `--native-codex` applies only to the tmux/SSH transport study.
The native Codex gate is `blocked` when no reachable authorized host has Codex.
Actual provider mouse interaction remains a manual acceptance check even when
the automated native gate passes.

## `tmux-runtime-isolation.sh`

This local study evaluates the selected host-runtime topology: one private
tmux server, socket, session, window, and pane per live Workstream. It starts
only fixed shell endpoints, proves sibling isolation and ordinary-tmux
noninterference, and measures a bounded idle-server cohort.

```console
spikes/tmux-runtime-isolation.sh --overhead-count 8 \
  --result /tmp/wsnav-runtime-isolation.json
```

The `TMUX`-scrubbed endpoint proves only namespace visibility for a shell. It
does not establish that a native Codex TUI should run with `TMUX` removed.
