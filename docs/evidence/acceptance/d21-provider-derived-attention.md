# D21 provider-derived attention acceptance

Date: 2026-09-02

Status: implemented in checkpoint `868ee85`, locally accepted, and installed
for operator inspection. D20 checkpoint `00a4937` is the starting boundary for
this checkpoint. This record claims no remote-CI or live Codex/OpenCode
interaction.

## Accepted boundary

- WSNav exposes no Navigator `a` action and no public `acknowledge` command,
  footer entry, help entry, controller action, revision fence, or result-seen
  mutation.
- Session-card completion is a direct projection of
  `RuntimeStatus::Attention` as `✓`. A subsequent observed user prompt changes
  the Runtime to `working` and therefore renders `●` without an acknowledgment
  transition.
- `!` is reserved for actual Workstream or onboarding recovery lifecycle. It
  is cleared only by the existing exact recovery flow, not by selection,
  activation, focus, attachment, or provider cycling.
- Current domain, snapshot, registry, and lifecycle code has no
  `AttentionState` or duplicated native session/turn identity. The exact
  provider binding and its accepted `last_settled_turn_id` remain the sole
  conversation-tip authority.
- Schema 15 is unchanged. The existing `attention_states` table and columns
  remain validated but are ignored historical storage: current snapshots and
  lifecycle observers neither read nor write them.
- The bounded diagnostic `operations` command and existing Runtime,
  Workstream, provider-binding, private-tmux, Park, archive, and recovery
  contracts are unchanged.

## Repository evidence

The final `scripts/check` passed on Rust 1.98.0 and tmux 3.7c. It ran strict
formatting and Clippy, 376 library tests, 10 presentation integration tests,
locked packaging, dependency advisory/license/source policy, shell and Python
checks, fixture validation, current-source and CLI acceptance, 43 focused
presentation tests, 32 focused current-state tests, validation of links in 56
Markdown files, and staged/unstaged diff checks.

Two earlier complete-gate runs stopped without being counted as acceptance.
The first exposed test-only Clippy `too_many_lines` findings after the new
compatibility coverage was added; those helpers were factored. The second
reached 375 of 376 library tests and exposed stale expected help-overlay
geometry after the acknowledge row was removed; the expectation was corrected.
The final complete gate passed.

Architecture review also found that removing the final attention write had
accidentally removed a provider turn-ID validation side effect from the Codex
Stop transaction. Explicit provider-metadata validation was restored before
the binding update. A transactional regression test proves newline-containing
and 257-byte turn IDs are rejected without changing the binding, Runtime,
Workstream, or legacy attention row.

Deterministic tests additionally prove that:

- acknowledgment CLI, action, key, footer, and help surfaces are absent;
- completion, working, starting, parked, recovery, unknown, and idle markers
  follow their exact current Runtime or Workstream lifecycle;
- legacy attention rows neither affect snapshots nor receive lifecycle writes;
  and
- schema-15 validation still requires the retained historical table and exact
  columns.

## Installed-artifact evidence

Before replacement, the installed `wsnav 0.1.0` had executable hash
`be8475a79227ae3304ae596858a6d3bba48535bc654c7042da8851b661063b06`,
reported no unresolved operations, and opened the existing schema-15 state.
Read-only inspection found one legacy attention row with an unseen-result
revision, no recovery revision, one `open` Workstream with an `attention`
Runtime, and zero unresolved historical Fork effects. The database hash was:

```text
287d1135904918ce7859210aa3d596f0891c72b778e1c31ba8cb69e6903ba21a  ~/.local/state/wsnav/host.sqlite
```

The locked release was built and atomically installed to
`~/.local/bin/wsnav`. The release and installed executable are byte-identical:

```text
3a83ddca0cbc67f048d5e0229c509b0f3548fda9fa47ca620c04c684e3b210a6  target/release/wsnav
3a83ddca0cbc67f048d5e0229c509b0f3548fda9fa47ca620c04c684e3b210a6  ~/.local/bin/wsnav
```

The installed binary reports `wsnav 0.1.0`, opens the existing state with an
empty `operations` result, and omits acknowledgment from public help. The
schema version, legacy row counts, Runtime/Workstream lifecycles, and database
hash remained identical after opening with the new binary; no migration,
reset, or rewrite occurred.

## Unclaimed evidence

- No live provider command, native lifecycle action, observer installation,
  trust change, ordinary tmux server, or provider pane was exercised.
- No remote CI or alternate Rust/tmux compatibility run is claimed.
- This checkpoint does not claim removal of the historical schema-15 table or
  broaden authority over provider conversations, content, or native history.
