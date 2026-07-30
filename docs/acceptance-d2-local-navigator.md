# D2 local navigator acceptance

Status: pass — 2026-07-29

D2 proves the first directly interactive local presentation without replacing
the native Codex workflow. It uses one disposable repository and state root,
one exact owned observer profile, private tmux servers only, and an explicit
native hook-trust review.

## Procedure

1. Build the checkout, create an empty disposable Git repository, and record
   the ordinary tmux fingerprint.
2. Run `wsnav setup` against the disposable state root. The operator reviews
   and trusts only the four exact `wsnav _hook` lifecycle entries in Codex's
   native UI, then runs `wsnav trust-observer`.
3. Register the disposable checkout and open bare `wsnav` from a terminal the
   operator controls. The navigator must render a bounded Workstream row.
4. Select the row through the navigator, which starts or attaches the exact
   private Runtime and shifts focus to the directly interactive native Codex
   pane. Complete one harmless native turn with a harmless image attachment.
5. Return to the navigator, verify visible result attention and unchanged
   native result output, then detach and rerun bare `wsnav` to reconnect to
   the same presentation. Close the navigator explicitly with `q`.
6. Park the exact Runtime, remove only the exact owned observer profile, remove
   the disposable filesystem root, and compare the ordinary tmux fingerprint.

## Recorded results

The final disposable native-Codex run passed with Codex CLI `0.146.0` and the
`codex-d2-private-presentation-v1` contract fingerprint.

- The operator explicitly completed the native hook review; no trust bypass or
  direct trust-store mutation was used.
- The navigator rendered one bounded local Workstream and attached its right
  pane to an unchanged, directly interactive native Codex TUI. Native
  repository trust was handled by Codex itself.
- The operator confirmed the normal native image attachment and harmless-turn
  flow, result-tip preservation, navigator attention visibility, and
  detach/reconnect behavior. Navigator status never appeared in the provider
  pane.
- The durable result attention was present without recovery attention. Closing
  the presentation left its Runtime recoverable until exact park.
- The private presentation was gone after `q`; the exact Runtime and observer
  profile were then removed, the disposable root was deleted, and the ordinary
  tmux fingerprint was unchanged.

An initial live interaction exposed an out-of-bounds provider-attachment
argument in the navigator. It was corrected before the final pass and now has
a direct regression test. The sanitized [fixture](../spikes/fixtures/d2-local-navigator.json)
contains only boolean assertions and the behavioral fingerprint; it commits no
identities, paths, prompts, responses, image data, terminal capture, PIDs,
credentials, or raw provider payloads.
