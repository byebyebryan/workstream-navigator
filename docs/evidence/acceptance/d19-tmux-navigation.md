# D19 tmux-derived navigation acceptance

Date: 2026-08-31

Status: implemented and locally accepted checkpoint `a0ec38b`. The full
repository gate, a clean Rust 1.88/tmux 3.3a compatibility run, the locked
release build, and byte-identical per-host installation passed. This record
does not claim remote CI or real Codex/OpenCode acceptance; D18 remains the
latest checkpoint with separately authorized live-provider lifecycle evidence.

## Candidate boundary

- Tmux is the only pane-focus authority. `Ctrl+b Left`/`Right` and validated
  primary-button press are the only ordinary focus transitions; Navigator
  activation and right-surface replacement preserve focus.
- Presentation and Runtime prefix/root tables are exact role-specific closed
  allowlists. Split, window, layout, menu, prompt, and arbitrary-command routes
  are absent from WSNav-owned interaction tables.
- Provider-pane `Ctrl+b Up`/`Down` selects the adjacent eligible already-live
  Workstream in the same activity-based visual order as Navigator. It skips
  ineligible rows, does not wrap, preserves right-pane focus, and causes no
  Start, Resume, recovery, Fork, Park, or other lifecycle effect.
- Focus remains ephemeral tmux state. The only new attachment metadata is
  bounded presentation-private purpose/attempt handshake data; no provider
  content, durable focus, or durable Navigator selection is stored.
- The state epoch remains schema 15 and ordinary/default tmux servers remain
  outside WSNav authority.

The design contract landed in `a271ac9`. The pre-implementation falsification
record landed in `e6cde73`; it proved that D18's permissive Runtime tables,
mutating attachment preflight, and ProjectId group order could not be reused.
Implementation `a0ec38b` replaces those assumptions with exact Runtime
topology/table validation, strict read-only cycle preflight, and one shared
activity-based Project/Workstream ordering authority.

## Repository and compatibility evidence

`scripts/check` passed on the development host with Rust 1.98.0 and tmux 3.7c.
It ran formatting, strict Clippy, 369 library tests, 8 presentation integration
tests, packaging, dependency license/advisory policy, shell/Python/fixture
checks, source and CLI acceptance, disposable presentation/state acceptance,
Markdown links, and staged/unstaged diff checks.

An exact local clone of `a0ec38b` in `rust:1.88-bookworm`, with tmux 3.3a,
passed:

```text
cargo test --locked --all-targets --all-features --quiet -- --test-threads=1
369 library tests passed
8 presentation integration tests passed
```

The container clone was deleted after the run. No ordinary WSNav state or
default tmux server was used.

Representative deterministic proof includes:

- live presentation mouse validation refuses changed topology before focus or
  delivery, and valid SGR press focuses and forwards while release/wheel over
  an inactive pane preserve focus;
- exact presentation and Runtime table inventories, unsafe-binding absence,
  topology refusal, and live Runtime table convergence without provider-process
  restart;
- literal nested `Ctrl+b` delivery through an outer tmux path;
- byte-preserving Codex read-only success/refusal and OpenCode missing-handle
  refusal, including unchanged DB/WAL bytes and record revisions;
- shared visual order, ineligible-row skipping, no-wrap behavior, and
  provider-cycle-only one-shot Navigator synchronization; and
- real presentation precommit rollback and success seams preserving status,
  marker, and focus at the outer-pane replacement boundary.

## Artifact and installation

`cargo build --locked --release` produced `wsnav 0.1.0`. The release was
copied to a temporary file in `~/.local/bin` and atomically renamed into place.
Source and installed artifacts compare byte-for-byte, are executable mode
`0755`, and share SHA-256:

```text
8c2517dab05ab64f7df720d3f4373b1c486e91ad176c8d7b791e740388251777
```

Installation is operator-inspection evidence, not publication or an accepted
live-provider release. No branch was pushed as part of this checkpoint.

## Evidence limits

No real Codex or OpenCode process was launched, no provider authentication was
copied, and no live-provider behavior is inferred from the installation. No
remote CI result is recorded.

The provider-cycle proof is deliberately composed rather than one full
two-Runtime fake-provider scenario: deterministic scanner/read-only-preflight
tests prove destination selection and absence of durable/provider effects,
while injected real-presentation respawn seams prove the serialized outer-pane
commit, rollback, focus, marker, status, and Navigator synchronization
boundaries. This is sufficient for the implemented control boundary but does
not constitute live-provider acceptance.
