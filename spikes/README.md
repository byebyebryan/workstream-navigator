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
the disposable servers only; this script does not independently exercise
provider mouse behavior.

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
The terminal-substrate decision combines this remote evidence with local Spike
0005 and the historical Python Phase 7F trial; there is no separate manual
terminal spike gate.

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

## codex-terminal-presentation.sh

This local study provides current-version native Codex terminal evidence. It
launches one real, isolated Codex TUI in the selected retained-TMUX runtime
configuration and attaches it directly to a separate two-pane private
presentation server.

~~~console
spikes/codex-terminal-presentation.sh \
  --result /tmp/wsnav-codex-terminal-presentation.json
~~~

The temporary CODEX_HOME receives only a mode-0600 copy of auth.json; ordinary
Codex configuration, hooks, sessions, skills, and logs are excluded. It submits
one harmless image-attached prompt with read-only sandboxing. The result covers
keyboard input, resize/focus, reconnect, result-tip preservation, and cleanup.
It does not independently render-test mouse, pixels, images, or clipboard
behavior; the combined terminal-substrate evidence is sufficient for the V1
design.

--debug-output writes before/after raw provider captures only at the explicit
path with mode-0600; normal runs retain no raw terminal content.

### Deferred terminal-fidelity studies

The accepted two-server presentation can still show minor Ghostty cursor
artifacts during high-churn native Codex rendering. This is non-blocking V1
polish, not a reason to weaken Runtime isolation or replace the native pane.
The current observation is recorded as a deferred study in the
[roadmap](../docs/roadmap.md#2026-08-02-deferred-terminal-fidelity-studies).

Before changing terminal settings or topology, use a disposable extension of
this presentation harness to compare the retained nested path with a direct
single-tmux baseline, exercise one fixed harmless typing/streaming sequence,
and record only aggregate visual, latency, control-plane, and cleanup results.
Any terminal-setting matrix must be private to the spike servers. Do not alter
the ordinary tmux server, Ghostty configuration, ordinary Codex home, or user
provider session. A topology alternative is admissible only if it retains every
V1 native-UI and private-Runtime invariant.

## codex-observer-profile.sh

This local study validates the proposed dedicated Codex observer profile and
passive lifecycle-hook authority boundary.

~~~console
spikes/codex-observer-profile.sh \
  --result /tmp/wsnav-codex-observer-profile.json
~~~

The harness uses a temporary CODEX_HOME with a synthetic base configuration
that disables hooks and a separately selected wsnav-observer profile that
enables only four spike-owned hooks. It drives Codex's native hook-trust prompt,
then proves SessionStart, UserPromptSubmit, Stop, and SessionEnd ordering in a
fresh native TUI. Unmanaged, forged-process, and stale-generation invocations
are rejected only after stdin is fully drained.

The test also proves that an ordinary launch without the profile produces no
observer events, profile install/remove ownership checks fail closed, the
ordinary tmux server is unchanged, and all private state is removed. It emits
sanitized relationship assertions only; UUIDs, prompts, transcripts, paths,
PIDs, credentials, raw hook payloads, and raw terminal captures are not
retained.

## codex-app-server-naming.py

This local study validates the short-lived Codex App Server metadata boundary
beside a separately running native TUI.

~~~console
spikes/codex-app-server-naming.py \
  --result /tmp/wsnav-codex-app-server-naming.json
~~~

The spike creates one disposable native Codex thread, then starts a fresh
`codex app-server --listen stdio://` process for each bounded metadata
operation. It proves exact summary reads, native `/rename` visibility,
App-Server rename persistence, native-process and screen stability, response
field filtering, and the complete V1 display-name fallback matrix.

The Python harness is non-installed spike machinery, not product code or a
runtime dependency. Its result is sanitized and the temporary Codex home,
repository, schemas, App Server processes, private tmux servers, and raw
provider data are deleted before evidence is emitted.

## codex-running-settled-fork.py

This local study validates the V1 Fork Workstream boundary beside a running
native source TUI.

~~~console
spikes/codex-running-settled-fork.py \
  --result /tmp/wsnav-codex-running-settled-fork.json
~~~

The source completes one harmless turn, starts a second turn containing a
long-running `sleep`, and remains untouched while the harness creates a clean
worktree from a recorded base commit. The spike submits one exact
`thread/fork(lastTurnId=...)` request, deliberately leaves its response unread,
reconciles one persisted destination without retry, and resumes that
destination in another native TUI.

The destination runs a divergent harmless turn from the new worktree while the
source command is still active. Raw identities, turns, prompts, responses,
paths, PIDs, captures, and generated schemas are private and deleted. The
Python harness is disposable evidence machinery, not a product dependency.

## codex-hook-ancestry-authority.py

This local study validates the direct-parent authority candidate used for
passive Codex lifecycle evidence. Its default `--transition clear` run is the
recorded [Spike 0010](../docs/evidence/spikes/0010-codex-hook-ancestry-authority.md).
The follow-up variants establish a deliberately narrow native `/new` result:

- `new` is [Spike 0011](../docs/evidence/spikes/0011-codex-native-new-rebinding.md),
  which falsifies the `SessionStart` changed-binding candidate.
- `new-prompt` is [Spike 0012](../docs/evidence/spikes/0012-codex-new-prompt-session-rotation.md),
  which falsifies first-destination-prompt session rotation.
- `new-inventory` is [Spike 0013](../docs/evidence/spikes/0013-codex-new-thread-inventory.md),
  which proves native `/new` creates a distinct thread but not its exact live
  TUI binding.

The last variation runs in a unique disposable workspace:

~~~console
spikes/codex-hook-ancestry-authority.py --transition new-inventory \
  --result /tmp/wsnav-codex-new-thread-inventory.json
~~~

It uses the same temporary Codex home, private tmux servers, forged-hook
rejections, and cleanup checks as the clear study. Its sanitized result records
only the selected transition, fixed source categories, and aggregate identity
rotation assertions; it contains no provider identifiers, prompts, output,
terminal data, paths, process IDs, or credentials.
