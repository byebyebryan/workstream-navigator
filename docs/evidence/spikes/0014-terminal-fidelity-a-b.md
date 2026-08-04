# Spike 0014: terminal-fidelity A/B baseline

## Hypothesis

The retained two-server presentation topology emits far more cursor
positioning and erase traffic to the terminal client than a direct single-tmux
baseline for identical provider output. This measurable amplification is the
leading candidate for the cursor artifacts observed during high-churn native
Codex typing and streaming, and a controlled A/B instrument can reproduce it
without an operator's eyes.

## Procedure and isolation

The automated local study (`spikes/codex-terminal-fidelity.sh`) uses a
mode-`0700` temporary root with three private tmux servers:

- one **runtime** server running a deterministic bounded streaming/typing
  workload on an alternate screen;
- one **presentation** server whose provider pane attaches to that runtime
  (the retained nested topology); and
- one **direct** server running the identical workload with no nesting as the
  baseline.

Both servers use the exact production tmux configuration: status off, mouse
on, `tmux-256color` default terminal, `COLORTERM=truecolor`, extended keys
with the `csi-u` format, and RGB/extended-key terminal features for
`xterm-ghostty` and `tmux-256color`. No Codex binary, auth cache, ordinary
configuration, hooks, sessions, or tmux server are used. The workload is a
pure synthetic emitter; its cursor behavior is the unit under test.

A `script`-driven client attached to the presentation server records the exact
byte stream a real terminal would receive during the workload, and a second
client records the direct server the same way. Both servers and all temporary
state are removed before the sanitized fixture is written.

## Observed contract

The sanitized [fixture][fixture] reports a stable, failing baseline on tmux
`3.7b` across repeated runs:

- nested-to-direct **cursor-motion** ratio ~2.4-2.6x (CSI cursor addressing,
  cursor up/down/forward/back, horizontal position);
- nested-to-direct **total CSI** ratio ~1.43x;
- nested-to-direct **bytes** ratio ~1.2x;
- nested-to-direct **cursor-visibility** (`CSI ?25 h/l`) ratio ~1.8x;
- cleanup completes and the ordinary tmux fingerprint is unchanged.

The exact numbers vary slightly per run (the capture window is bounded by
wall-clock sleep), but the ordering is stable: the nested presentation always
re-emits the cursor churn. Status is `falsified:
nested-presentation-cursor-emission-amplified` because the amplification
exceeds the recorded bounds (`motion <= 1.5x` and `bytes <= 1.3x`).

## Decision and limits

The instrument is the deliverable: it reproduces the artifact class
deterministically, measures it objectively, and will confirm whether a
presentation change reduces the amplification without requiring an operator to
inspect the screen. It does not yet prove a root cause.

A probe removing the `c0ce139` extended-keys/terminal-features lines changed
the motion ratio negligibly (2.46 -> 2.42), so that recent configuration is
not the cause of the amplification; it is inherent to the nested redraw path.

The result is tmux-`3.7b`-specific and uses a synthetic workload rather than a
live Codex TUI, so it does not by itself prove a visual fix. Any candidate
must still retain normal input/resize/reconnect/result-tip behavior and keep
the native provider pane untouched; those remain the existing presentation and
native acceptance gates. This spike neither approves a presentation redesign
nor relaxes the deferred terminal-fidelity invariants.

[fixture]: ../../../spikes/fixtures/codex-terminal-fidelity.json
