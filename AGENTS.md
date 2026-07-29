# Workstream Navigator Development Instructions

## Authority

- `docs/design.md` is the V1 product and architecture contract.
- `docs/roadmap.md` owns delivery order, checkpoint scope, exit gates, and
  implementation status.
- If implementation evidence contradicts a core design invariant, stop and
  record the falsification instead of silently weakening the boundary.

## Product invariants

- Preserve the provider's native terminal UI and native workflow.
- Never write navigator status or management traffic into the provider pane.
- Preserve completed provider output until the user acts.
- Keep each live Runtime on its own private tmux server; never use or mutate the
  user's default tmux server.
- Store only the provider identifiers and bounded metadata required for exact
  operation. Never persist prompts, responses, tool output, terminal captures,
  transcripts, credentials, or raw provider payloads.
- Treat hooks and process observations as evidence, not mutation authority.
- Fail closed on ambiguous identity, ownership, revision, or external effects.
- Do not add compatibility behavior for the frozen Python prototype.

## Implementation discipline

- Implement only the active roadmap checkpoint; keep later and deferred scope
  out of the current change.
- Keep the CLI entrypoint thin and place behavior behind testable module
  boundaries.
- Prefer concrete Codex behavior over speculative multi-provider abstractions.
- Use typed IDs, explicit state transitions, transactional revisions, bounded
  I/O, and deterministic test seams.
- The first production-dependency commit must add and enforce its
  license/advisory policy in CI.
- Preserve unrelated user changes and commit coherent capabilities separately.

## Testing and validation

- Automated tests must use disposable state roots, repositories, Codex homes,
  and private tmux sockets. They must not install hooks or launch against
  ordinary user state.
- Live Codex or remote-host acceptance requires explicit operator intent,
  sanitized evidence, and complete cleanup.
- Run `scripts/check` before every checkpoint commit.
