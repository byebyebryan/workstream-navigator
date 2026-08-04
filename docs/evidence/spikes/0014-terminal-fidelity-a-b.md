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

## Follow-up: root cause is upstream tmux, not WSNav configuration

Follow-up probing with the instrument identified the root cause as **upstream
tmux behavior**: on every full client redraw, tmux emits `civis` (`CSI ?25 l`)
before synchronized output and `cnorm` (`CSI ?25 h`) after it, even when the
pane cursor is visible. On terminals where cursor-state updates restart
blinking (Ghostty included), repeated redraws during streaming visibly disrupt
the blink phase. This is [tmux issue
5419](https://github.com/tmux/tmux/issues/5419), filed against `next-3.8` and
not fixed in any tmux currently available on Arch (`3.7b` is the latest
released; the AUR `tmux-git` package is stale and upstream master lacks the
fix).

The following WSNav-controllable candidates were each ruled out with the
instrument; none changed the `civis`/`cnorm` emission:

- `set -g cursor-style block` (steady, non-blinking) - the option only selects
  the cursor shape; the hide/show toggle during redraw is independent and
  hardcoded in tmux's redraw path;
- `set -g extended-keys always` / `terminal-features` (commit `c0ce139`);
- `set -g update-scroll-region on`; and
- the `sync` (`CSI ?2026`) terminal feature, which is already active for
  Ghostty clients.

The artifact is therefore version-bound and not config-fixable by WSNav. The
instrument remains the objective confirmation gate for an upstream fix. See
the [roadmap](../../roadmap.md#2026-08-04-terminal-fidelity-root-cause-is-upstream-tmux)
for the deferred-fix decision.

## Decision and limits

The instrument is the deliverable: it reproduces the artifact class
deterministically, measures it objectively, and will confirm whether a
presentation change reduces the amplification without requiring an operator to
inspect the screen. The follow-up section above records the confirmed root
cause (upstream [tmux issue 5419](https://github.com/tmux/tmux/issues/5419))
and the ruled-out WSNav configuration candidates.

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
