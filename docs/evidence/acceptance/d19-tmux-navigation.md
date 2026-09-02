# D19 tmux-derived navigation acceptance

Date: 2026-08-31

Status: implemented and locally accepted checkpoint `a0ec38b`, with the
`fd9db8a` focus-and-frame refinement and current post-rename startup-ordering
and reconciliation-guidance corrections. The current source passes the full
local/disposable repository gate and byte-identical per-host installation. This
record does not claim remote CI or real Codex/OpenCode acceptance; D18 remains
the latest checkpoint with separately authorized live-provider lifecycle
evidence.

The current follow-up removes the visually heavy `ACTIVE`/`INACTIVE` pane
header. The presentation instead forwards tmux focus events, and the Navigator
uses them only to keep its page title green while focused and dark gray while
inactive. The title color is ephemeral rendering state, not focus authority or
durable UI state, and no cue is written into the provider pane.

The follow-up wraps the entire Navigator, including its footer, in one
continuous green frame. The adjacent tmux pane-boundary column uses a white
foreground and default background in both focus states, and half-border
indicators are disabled. The divider therefore remains one consistent native
tmux line. The compact footer also omits `↑↓` selection, `Enter` open/shell,
and `a` acknowledge-result hints while the complete `?` reference retains them.

## Candidate boundary

- Tmux is the only pane-focus authority. `Ctrl+b Left`/`Right` and validated
  primary-button press are the only ordinary focus transitions; Navigator
  activation and right-surface replacement preserve focus.
- The Navigator page-title color follows terminal focus gain/loss. There is no
  separate pane-focus header, tmux query loop, provider-pane traffic, or action
  authority attached to the color.
- One green outer frame contains the Navigator list and footer. The adjacent
  tmux boundary uses identical white active/inactive foregrounds and no forced
  background; disabled half-border indicators prevent a split divider.
- Presentation and Runtime prefix/root tables are exact role-specific closed
  allowlists. Split, window, layout, menu, prompt, and arbitrary-command routes
  are absent from WSNav-owned interaction tables.
- Fresh presentation startup retries only the exact transient
  `InvalidTopology` observation while tmux publishes its second pane, for at
  most 20 observations separated by 5 ms. Before Navigator can run, startup
  creates its pane with an inert internal command, captures the private socket
  identity into the ownership marker, role-marks the exact pane, and replaces
  its command with Navigator. Each provider-topology attempt revalidates the
  owned context and complete two-pane topology before the first control
  mutation; every other error and persistent ambiguity remains a refusal.
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

The original `a0ec38b` `scripts/check` passed on the development host with Rust
1.98.0 and tmux 3.7c. It ran formatting, strict Clippy, 369 library tests, 8
presentation integration tests, packaging, dependency license/advisory policy,
shell/Python/fixture checks, source and CLI acceptance, disposable
presentation/state acceptance, Markdown links, and staged/unstaged diff checks.

An exact local clone of `a0ec38b` in `rust:1.88-bookworm`, with tmux 3.3a,
passed:

```text
cargo test --locked --all-targets --all-features --quiet -- --test-threads=1
369 library tests passed
8 presentation integration tests passed
```

The container clone was deleted after the run. No ordinary WSNav state or
default tmux server was used.

Before the startup closure, the focus-and-frame refinement plus compact-footer
tightening passed `scripts/check` on Rust 1.98.0/tmux 3.7c with 372 library and
8 presentation integration tests. That workspace was then mounted read-only
into `rust:1.88-bookworm`; Rust 1.88.0 and tmux 3.3a passed the same 380 locked
all-targets/all-features tests. The container was removed after the run.

Before the current startup closure, several visual-refinement compatibility
runs passed every library test, then a presentation integration fixture refused
its initial provider-pane topology before reaching the behavior under test.
Each exact fixture passed on an isolated retry, and a fresh complete run after
each refusal passed the entire matrix. Those refusals were fail-closed and
created no provider effect; no interrupted matrix is counted as a passing gate.
They are retained here as the falsification that motivated the narrow bounded
startup policy rather than erased by the fix.

The current focused presentation target passes 10/10 tests on tmux 3.7c. Its
new startup fixture completes 16 consecutive fresh detached presentations and
requires exact live `navigator`/`provider` roles plus the complete closed prefix
table after every return. Its real-client focus fixture observes Navigator as
initially active and green, provider active with a dark-gray Navigator title
after `Ctrl+b Right`, and Navigator active and green again after `Ctrl+b Left`.
Only the Navigator pane's bounded output is inspected, under a disposable state
root and private tmux socket; provider output is neither captured nor written.

The final startup/focus candidate passed `scripts/check` on Rust 1.98.0/tmux
3.7c with 372 library and 10 presentation integration tests, plus formatting,
strict Clippy, packaging, dependency policy, source/CLI acceptance,
presentation/state acceptance, Markdown links, and staged/unstaged diff checks.
The current workspace was then mounted read-only into
`rust:1.88-bookworm`; Rust 1.88.0, tmux 3.3a, and zsh 5.9 passed all 382 locked
all-targets/all-features tests serially. An earlier under-provisioned container
without zsh stopped at the two explicit account-shell preconditions after
370/372 library tests; it is not counted as a passing gate. Each container was
removed after its run.

Representative deterministic proof includes:

- live presentation mouse validation refuses changed topology before focus or
  delivery, and valid SGR press focuses and forwards while release/wheel over
  an inactive pane preserve focus;
- a real attached tmux client drives initial/Right/Left focus and observes the
  Navigator title's ordered green/dark-gray/green terminal output without
  polling tmux from the product or inspecting the provider pane;
- 16 consecutive fresh detached starts return with the exact two live roles and
  complete closed prefix table, while deterministic retry tests prove transient
  success, persistent refusal, and immediate unrelated-error refusal;
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

## Post-rename documentation reconciliation

After repository rename `7651c12`, the current documentation pass removes the
stale duplicated pre-rename artifact identity and adds D19 to the roadmap's
completed-checkpoint index. A proposed fixed-illustration current hero was
reviewed and rejected before publication because it no longer matched the
chosen real-terminal capture direction. No generated current hero or capture
tool is part of this candidate; the committed pre-D16 media remains explicitly
historical while current-product capture waits for the next UI/UX checkpoint.

### Post-rename startup falsification and closure

Before the startup-ordering correction, the exact post-rename source passed
ordinary non-interactive `scripts/check` with 373 library and 10 presentation
integration tests, formatting, strict Clippy, packaging, dependency policy,
source/CLI and presentation/state acceptance, Markdown links, and
staged/unstaged diff checks.

The required source-read-only `rust:1.88-bookworm` matrix did not pass. With
Rust 1.88.0, tmux 3.3a, and zsh 5.9, two serial locked
all-targets/all-features runs each passed all 373 library tests, then failed the
same `repeated_fresh_presentations_publish_exact_controls_before_returning`
integration test at fresh start 12 and fresh start 0 respectively. Both
failures were `StartupFailed { stage: "provider pane setup", source:
InvalidTopology }`. A fresh container running only that exact 16-start test
passed twice, then reproduced the same failure at fresh start 5 on its third
run.

The failures were fail-closed and used only disposable state plus private tmux
sockets, but they contradicted the bounded startup-retry acceptance claim. A
longer provisional retry was tested and rejected: even after 100 observations
over roughly 500 ms, a later exact run showed the complete 32/96 two-pane
geometry and correct roles while Navigator was dead with normal status 1. The
provider-topology observation was therefore downstream evidence, not the
cause.

The exact cause was a separate ownership publication race. Tmux launched
`_navigator` as part of `new-session`, while the parent immediately rewrote the
private ownership marker to bind the newly created socket identity. Navigator
could perform its first context proof during that rewrite, correctly refuse
the changing marker, and exit. The correction starts pane `0.0` with the inert
internal `_provider_wait`, captures the socket identity, sets the exact
Navigator role and `remain-on-exit`, and then uses exact-target `respawn-pane`
to launch `_navigator`. It does not retry marker ambiguity, adopt or accept a
dead pane, weaken topology, or write into the provider pane. The independent
20-observation provider-pane publication retry remains unchanged.

Deterministic tests prove Navigator launch observes the already-published
socket identity and that respawn targets only the bootstrap pane. On tmux 3.7c,
five focused fixtures passed 80 fresh starts before the full repository gate.
In a fresh `rust:1.88-bookworm` environment with Rust 1.88.0, tmux 3.3a, and
zsh 5.9, five focused fixtures passed 80 starts and five uninterrupted serial
locked all-targets/all-features matrices passed, including another 80 fresh
starts. No failed or interrupted run is counted. The ordinary final gate also
passed formatting, strict Clippy, packaging, dependency policy, source/CLI and
presentation/state acceptance, Markdown links, and staged/unstaged diff checks.
The earlier fail-closed failures remain historical and are not reclassified. No
new remote-CI or live-provider claim is made.

### Reconciliation-guidance recovery

An operator capture exposed a separate presentation-local defect: one transient
post-exec reconciliation failure set `Managed session reconciliation is
unavailable; exact recovery required`, but later exact success and completed
marker retirement did not clear it. Durable state and provider-exec proof had
already converged; only the Navigator's process-local guidance remained stale.

The correction centralizes the bounded message and applies exact
compare-and-clear behavior. Reconciliation failure still shows the fail-closed
warning. Successful exact proof or the following normal idle state clears only
that same warning, while newer unrelated guidance remains visible. Deterministic
tests cover unavailable and failed refreshes, successful recovery, completed
marker retirement, and unrelated-guidance preservation. The change adds no
provider I/O, durable field, retry, compatibility behavior, or broader recovery
authority.

The final local `scripts/check` passes 377 library and 10 presentation
integration tests, formatting, strict Clippy, packaging, dependency policy,
source/CLI and presentation/state acceptance, 54 Markdown files, and both diff
checks. The exact Rust 1.88 startup evidence above remains bound to the source
that produced it; no renewed compatibility-matrix or live-provider claim is
transferred to the guidance-only follow-up.

## Artifact and installation

The final pre-rename D19 focus-and-frame candidate produced `wsnav 0.1.0` at
SHA-256
`46365fe25fe0edacc728f4f1269487a24671a4ad90695264db2e70ed55e26b2c`.
That value remains historical evidence for that exact source; it no longer
describes the post-rename installation.

### Current startup-ordering and reconciliation-guidance artifact

`cargo build --locked --release` produced `wsnav 0.1.0`. The release was copied
to a temporary file in `~/.local/bin` and atomically renamed into place. Source
and installed artifacts compare byte-for-byte, are executable mode `0755`, are
7,463,184 bytes, and share SHA-256:

```text
ea5a6711476919bdd47aa9834e9dc6ed8529f02f2229b08db2931cc21cebed6c
```

Installation and source publication are operator-inspection evidence, not an
accepted live-provider release.

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
