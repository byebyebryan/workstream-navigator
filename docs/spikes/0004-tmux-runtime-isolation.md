# Spike 0004: per-Workstream tmux runtime isolation

## Hypothesis

Workstream Navigator can give every live Workstream its own private tmux
server, socket, single session, single window, and single provider pane. This
keeps the user's ordinary tmux namespace clean while avoiding shared-server
failure, sizing, attachment, and session-list coupling between Workstreams.

## Procedure and isolation

The local harness starts fixed shell endpoints only. It never launches Codex,
opens SSH, creates a repository, or changes a user tmux server. Every managed
operation uses an absolute private `tmux -S <socket>` path with the caller's
`TMUX` environment removed. The ordinary tmux server is read only for a
sanitized before/after fingerprint.

The run creates two sibling runtime servers:

```text
runtime A private server -> one session -> one window -> one pane
runtime B private server -> one session -> one window -> one pane
```

Runtime A retains `TMUX`; a bare `tmux list-sessions` inside its endpoint must
see exactly its own private session. Runtime B removes `TMUX`; the same command
must not discover B's managed session through the ordinary server. The harness
then stops A, proves that B retains the same server process and input path, and
starts eight additional fixed endpoints to measure bounded idle-server overhead.

All directories are mode `0700`; sockets and processes are removed before the
result is emitted. The fixture contains only booleans, the server count, total
RSS, timing, and cleanup status.

## Observed result

The sanitized [fixture][fixture] passed locally with tmux `3.7b`:

- each runtime used a distinct private server and socket;
- every server contained exactly one session, one window, and one pane;
- the inherited-`TMUX` runtime saw exactly its own session, while the scrubbed
  shell endpoint did not discover its managed session through bare `tmux`;
- stopping runtime A left runtime B's server process and input round trip
  intact;
- eight additional idle tmux servers consumed `47,036 KiB` total RSS in this
  run, about `5.9 MiB` per server; and
- the ordinary tmux fingerprint was unchanged and cleanup completed.

The overhead figure is a local diagnostic, not a capacity guarantee. It is
small relative to a live coding-agent process and is measured only for tmux
servers with fixed shell endpoints.

## Decision and limits

V1 should use one private tmux server per live Workstream runtime, not a
shared host server with many Workstream sessions and not many Workstreams as
windows of one session. The local presentation remains a separate disposable
private server with its navigator and attachment panes.

Removing `TMUX` is unnecessary for the selected topology: a native TUI that
retains it sees at most its own bounded private session. The subsequent
[terminal-presentation spike][spike-0005] accepted retained `TMUX` as the V1
configuration and proved native color, mouse configuration, image attachment,
resize, focus, reconnect, and result-tip preservation. The earlier
[native Codex transport spike][spike-0002] independently established that an
actual Codex TUI survives private-tmux detach and reconnect.

[fixture]: ../../spikes/fixtures/tmux-runtime-isolation.json
[spike-0002]: 0002-codex-native-tui.md
[spike-0005]: 0005-codex-terminal-presentation.md
