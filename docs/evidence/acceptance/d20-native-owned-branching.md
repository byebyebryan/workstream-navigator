# D20 native-owned conversation branching acceptance

Date: 2026-09-02

Status: implemented in checkpoint `00a4937`, locally accepted, and installed
for operator inspection. Provider-owned naming refinement `b3a58bb` is the
starting boundary for this checkpoint. This record claims no remote-CI or live
Codex/OpenCode interaction.

## Accepted boundary

- WSNav exposes no Navigator `f`/Fork or `r`/Fork-recovery action and no public
  `fork-workstream` or `recover-operation` command.
- The Codex adapter no longer sends `thread/fork` or reconciles a lost managed
  Fork result. The OpenCode adapter no longer sends its Fork mutation.
- Native provider conversation branching remains provider-owned. When exact
  lifecycle evidence proves a supported native session cutover, the existing
  binding logic rotates the current tip on the same Workstream; it does not
  create another card. Navigator `n` remains the distinct blank-Workstream
  action at the selected Location.
- The generic compound-operation journal remains for onboarding and OpenCode
  blank-session Start. The public `operations` command remains a bounded
  diagnostic for unresolved non-onboarding creation operations, not a recovery
  surface.
- Schema 15 and the private-tmux topology are unchanged. Historical
  `origin='fork'` Workstreams plus completed or failed Fork journal rows remain
  readable and inert.
- A historical Fork journal in `external_effect_started`,
  `awaiting_reconciliation`, or `recovery_required` makes current state open
  fail with `RetiredForkRecoveryRequired`. The refusal does not mutate the
  database or retry, infer, adopt, or delete a provider effect; the previous
  accepted build must resolve it first.

## Repository evidence

The final `scripts/check` passed on Rust 1.98.0 and tmux 3.7c. It ran strict
formatting and Clippy, 375 library tests, 10 presentation integration tests,
locked packaging, dependency advisory/license/source policy, shell and Python
checks, fixture validation, current-source and CLI acceptance, disposable
presentation/state acceptance, Markdown-link validation, and staged/unstaged
diff checks.

The first complete gate stopped at Clippy after the capability record shrank:
`capability_is_well_formed` still accepted the now-seven-byte copy type by
reference. The helper was corrected to accept the value directly; that failed
run is not counted as acceptance. The final complete gate passed.

Deterministic state tests additionally prove that:

- all three unresolved historical Fork effect phases refuse with the typed
  error while database bytes and journal rows remain unchanged;
- completed and failed Fork journal history plus Fork-origin Workstreams open
  without being rewritten or projected as recovery work; and
- a supported native `SessionStart(source=clear)` transition changes the exact
  provider binding and predecessor on one existing Runtime and Workstream.

## Installed-artifact evidence

Before replacement, the installed `wsnav 0.1.0` reported no unresolved
operations. Direct read-only schema-15 inspection found one completed
`onboard/provider_exec_proven` journal row, zero unresolved Fork effects, and
one independent-origin Workstream; no state mutation or reset was needed.

The locked release was built and atomically installed to
`~/.local/bin/wsnav`. The release and installed executable are byte-identical:

```text
be8475a79227ae3304ae596858a6d3bba48535bc654c7042da8851b661063b06  target/release/wsnav
be8475a79227ae3304ae596858a6d3bba48535bc654c7042da8851b661063b06  ~/.local/bin/wsnav
```

The installed binary reports `wsnav 0.1.0`, opens the existing state with an
empty `operations` result, and omits both retired commands from public help.

## Unclaimed evidence

- No live provider command, native `/fork`, observer installation, trust
  change, ordinary tmux server, or provider pane was exercised.
- No remote CI or alternate Rust/tmux compatibility run is claimed.
- This checkpoint does not broaden the existing exact native session-cutover
  contract or infer a new session from provider inventory or ordering.
