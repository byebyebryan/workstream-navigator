# Spike 0005: native Codex two-pane terminal presentation

## Hypothesis

Workstream Navigator can retain TMUX inside an isolated native Codex runtime
while presenting that runtime directly in a second pane of a separate private
tmux server. A completed provider result tip must survive focus changes,
presentation resize, presentation-server destruction, and reconnection without
restarting Codex or sending any post-result provider input.

## Procedure and isolation

The local harness creates a mode-0700 temporary root containing:

- one private runtime tmux server, session, window, and Codex pane;
- one separate private presentation server with a fixed navigator placeholder
  pane and a direct nested tmux attachment to the runtime;
- an empty disposable workspace and a CODEX_HOME that contains only a
  mode-0600 copy of the existing Codex auth.json; and
- a generated one-pixel image attached to the first native composer prompt.

The test launches Codex with read-only sandboxing and no ordinary
configuration, hooks, sessions, skills, or logs. It accepts the first-use trust
prompt only for that empty workspace, sends one fixed harmless prompt as
separate text and Enter events, waits for the exact response marker, changes
focus, resizes the presentation, destroys only the presentation server, and
recreates it at the same dimensions.

The runtime server and Codex process remain alive throughout the reconnect.
The harness compares the isolated response-tip line before and after
reconnection. It never sends provider input after the result completes.
Temporary processes, sockets, workspace, temporary home, and image are removed
before the sanitized result is emitted. An optional raw diagnostic capture is
mode-0600 and opt-in only.

## Observed result

The automated study passed locally with Codex CLI 0.145.0; see the sanitized
[fixture][fixture].

- both tmux servers used tmux-256color, COLORTERM=truecolor, and mouse on;
- the native runtime retained its private TMUX environment;
- the presentation had exactly two panes and one direct client attachment to
  the one-pane runtime;
- native keyboard submission completed the harmless image-attached turn;
- presentation resize and focus changes reached the native TUI;
- the native Codex pane process was unchanged across presentation detach and
  reconnect;
- after presentation reconnect, the exact provider response-tip line was
  unchanged; and
- the ordinary tmux fingerprint was unchanged and cleanup completed.

An initial whole-screen comparison was intentionally rejected: Codex recomputed
two prompt/status-chrome lines after reattachment even though the provider
result was unchanged. The accepted assertion therefore compares the completed
provider result tip, not volatile TUI chrome. It does not claim byte-identical
full-screen rendering.

## Decision and limits

The selected V1 default is viable: retain the private TMUX environment for a
native Codex runtime and attach it directly to a separate private presentation
server. No TMUX-scrubbed Codex configuration is needed to keep ordinary tmux ls
clean or bound the runtime namespace.

This automated result, together with the historical [Python Phase 7F terminal
trial][phase-7f], is sufficient terminal-substrate evidence for the V1 design.
That trial confirmed direct native-pane interaction beside a navigator,
tmux-256color and truecolor behavior, click-to-select mouse support, and
result-line stability in an equivalent private-tmux layout.

The Python prototype remains behavioral evidence only: it is frozen reference
material, not a product release, implementation dependency, or compatibility
constraint for Rust. The eventual Rust navigator still needs normal
product-level end-to-end testing, but visual terminal review is not a
pre-implementation or spike gate.

[fixture]: ../../spikes/fixtures/codex-terminal-presentation.json
[phase-7f]: https://github.com/byebyebryan/agent-switchboard-python-reference/blob/main/docs/phase-7f-acceptance.md
