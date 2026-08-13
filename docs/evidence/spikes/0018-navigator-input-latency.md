# Spike 0018: Navigator input-delivery and echo-latency A/B

## Hypothesis

The operator-visible delay while typing may be either actual key delivery to
the native pane or delayed visual echo after the provider has received the
key. A synthetic nested-tmux study can measure those boundaries separately and
establish whether the Navigator's former 10 FPS animation alone reproduces the
lag on the local terminal path.

## Procedure and isolation

The automated study (`spikes/navigator-input-latency.py`) creates two fresh
cases beneath one mode-0700 temporary root. Each case uses one private Runtime
tmux server containing a synthetic raw-mode endpoint and one private
presentation tmux server containing a narrow Navigator pane beside a nested
attachment to that Runtime. The provider pane remains active throughout the
measurement.

The static case draws its Navigator once. The comparison case performs the
same full-pane synthetic redraw every 100 ms. A PTY-attached presentation
client sends 90 bounded sequence tokens over approximately four seconds. The
endpoint records a monotonic timestamp immediately on receipt and returns it
in an acknowledgement. The harness records client send time, endpoint receive
time, and client observation time, then retains only aggregate input-delivery
and echo latency.

No coding-agent binary, authentication, provider configuration, WSNav state,
real provider pane, prompt, response, or network connection is used. The
ordinary tmux server is read only for its before/after session fingerprint and
is never mutated. Raw client bytes, sequence tokens, paths, process identities,
and temporary files are discarded. Cleanup verifies that both private servers
are gone and the ordinary tmux session fingerprint is unchanged.

## Observed contract

The sanitized [fixture][fixture] passes on tmux 3.7b:

- all 90 samples returned in each case;
- the static case measured 0.385 ms p95 input delivery and 0.557 ms p95 echo;
- the 10 FPS case measured 0.371 ms p95 input delivery and 0.490 ms p95 echo;
- static input and echo remain far below the respective 25 ms and 50 ms
  acceptance bounds; and
- cleanup completed with the ordinary tmux fingerprint unchanged.

The observed animated/static p95 ratios were 0.964 for delivery and 0.880 for
echo. This local run therefore did not reproduce added latency from animation
alone. The result does not contradict the separately measured tmux redraw and
cursor-visibility amplification: it says only that the synthetic local PTY
path stayed responsive during this bounded workload.

## Decision and limits

D8.11 may remove the animated marker to eliminate avoidable redraw churn, but
must not claim that animation alone caused the reported typing lag. The other
observed hot paths—repeated executable probes and event-frequency OpenCode HTTP
corroboration—remain independent latency candidates and are corrected by the
same checkpoint.

This is local synthetic evidence. It does not exercise SSH latency, network
backpressure, Ghostty rendering time, Codex/OpenCode input processing, or a
real provider's event loop. Operator-gated native use remains the final
acceptance boundary for perceived typing responsiveness.

[fixture]: ../../../spikes/fixtures/navigator-input-latency.json
