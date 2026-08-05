# Spike 0017: OpenCode blank-session binding and observer ownership

## Question

Can WSNav create an OpenCode Workstream from a blank provider session, launch
the native TUI without WSNav-owned model or prompt flags, and supervise one
exact observer sidecar per Runtime without crossing sessions or roots?

## Procedure and isolation

The disposable, operator-gated probe is
[`spikes/opencode-fresh-session.py`](../../../spikes/opencode-fresh-session.py).
It was run against OpenCode `1.18.11`. The harness creates isolated XDG
configuration/data/cache/state roots, copies only the existing mode-0600 auth
file into the temporary data root, and creates separate temporary project
roots. It uses only private tmux sockets and removes provider state, observer
processes, tmux servers, open loopback endpoints, and temporary roots before
writing the result.

The probe:

1. starts a short-lived OpenCode server and creates two blank sessions through
   `POST /session`, confirming that neither contains conversational records;
2. stops that server and launches two native TUIs at the same project root
   with `opencode <root> --hostname 127.0.0.1 --port <port> --session <id>`;
3. verifies the exact session IDs, OpenCode version, private tmux pane process
   birth, and loopback socket ownership by the pane process or its descendants;
4. starts one separate stdio-disconnected observer child per Runtime before
   native input, retaining only bounded event counts and lifecycle metadata;
5. sends harmless marker prompts through each native composer and confirms
   prompt/event separation, unrelated-root rejection, child-session filtering,
   wrong-endpoint rejection, explicit port-collision rejection, and stale-port
   rejection;
6. terminates and replaces one observer, simulates a detached/reopened tmux
   attachment, parks the first Runtime, and resumes its exact session in a new
   Runtime generation; and
7. writes only aggregate assertions and one-way session/generation digests to
   [`opencode-fresh-session.json`](../../../spikes/fixtures/opencode-fresh-session.json).

The marker prompts are operator-approved probe input delivered through the
native TUI. They are not part of the production launch command and are not
retained in the fixture.

## Result

The [sanitized fixture][fixture] is a pass:

- blank sessions created by the short-lived server bind exactly to two native
  TUIs at the same ProjectLocation;
- the production command shape uses no `--pure`, `--model`, `--agent`, or
  `--prompt` overrides;
- each loopback endpoint correlates to its exact provider pane process tree,
  while a healthy unrelated endpoint and an occupied port are rejected;
- each observer records its Runtime generation and process birth, filters to
  its exact session, and discards child/unrelated session evidence; the
  observer role can be replaced after a helper crash and remains independent
  of tmux detach/reopen; and
- the exact session resumes after its Runtime is parked and restarted.

The result passes on the explicitly allowlisted OpenCode `1.18.11` version.

## Design consequence and limits

The selected fresh-binding mechanism is **blank provider-session precreation**:
the host may use a short-lived OpenCode server to obtain an exact blank session
ID, stop that server, and then launch the native TUI with that ID. A production
adapter must preserve the same no-prompt/no-model launch shape and must keep
the endpoint, pane process birth, observer PID/birth, and Runtime generation
host-private.

This spike does not prove OpenCode-native session creation or switching inside
an already managed TUI. No exact active-TUI changed-binding claim is adopted;
the fail-closed boundary remains the same as Codex native `/new`: WSNav uses a
new Workstream for another conversation. Navigator Rename and OpenCode Fork
remain separate capability gates.

This is behavioral evidence for OpenCode `1.18.11`, not a production adapter,
provider onboarding, transcript ingestion, or evidence for any later OpenCode
version. Re-run it after an upgrade before expanding the version allowlist.

[fixture]: ../../../spikes/fixtures/opencode-fresh-session.json
