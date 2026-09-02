# D22 exact live recovery confirmation acceptance

Date: 2026-09-02

Status: implemented in checkpoint `ed74b0b`, locally accepted, and installed
for operator inspection. D21 checkpoint `868ee85` is the starting boundary for
this checkpoint. This record claims no remote-CI or live Codex/OpenCode
interaction.

## Accepted boundary

- Explicit Recover recognizes only a non-archived Codex Workstream in
  `recovery_required` whose exact retained ProviderBinding belongs to an older
  generation of the same `starting` Runtime.
- Before any durable mutation, recovery proves the exact private tmux topology,
  pane PID, process birth, cwd, an absolute executable ending in `codex`, and
  the complete generated
  `codex --profile wsnav-observer -C <cwd> resume <retained-session>` argument
  vector.
- One bounded ephemeral App Server performs only
  `thread/read(includeTurns=false)` for the retained native ID. The returned ID
  must match, and the Runtime topology and process command are proved again
  after the provider read.
- A single immediate transaction revalidates Workstream, Runtime, provider,
  binding, native session, generation, lifecycle, archive, and revision fences.
  Success advances only the binding generation/revision and Workstream
  lifecycle/activity/revision. Runtime status and revision remain `starting`
  and unchanged until native lifecycle evidence advances them.
- Any unavailable, malformed, stale, racing, or mismatched evidence fails
  closed without state mutation. The Navigator retains `!` and reports bounded
  guidance outside provider content.
- Initial binding, changed-session transition, unbound/native-picker recovery,
  session-list inference, and OpenCode recovery remain unchanged. The path does
  not read a provider pane or stop, restart, signal, steer, or send input to a
  provider. Schema 15 is unchanged.

## Repository evidence

The final `scripts/check` passed on Rust 1.98.0 and tmux 3.7c. It ran strict
formatting and Clippy, 380 library tests, 10 presentation integration tests,
locked packaging, dependency advisory/license/source policy, shell and Python
checks, fixture validation, current-source and CLI acceptance, 43 focused
presentation tests, 32 focused current-state tests, validation of links in 57
Markdown files, and staged/unstaged diff checks.

Focused tests prove:

- exact retained-session process and thread evidence reconciles successfully;
- PID, birth, cwd, executable, argv, and thread mismatches preserve state;
- a topology change after `thread/read` is caught by the second probe before
  mutation; and
- unbound, provider, session, Runtime/binding generation, Runtime lifecycle,
  archive, and every Runtime/Workstream/binding revision mismatch fails without
  a partial transaction.

Architecture review found and closed a time-of-check/time-of-use gap in the
first candidate: process evidence had originally been proved only before the
bounded provider read. The accepted candidate re-proves topology, PID, birth,
cwd, executable, and argv after that read and before its transactional state
fence.

## Installed-artifact evidence

Before replacement, the installed `wsnav 0.1.0` had executable hash
`3a83ddca0cbc67f048d5e0229c509b0f3548fda9fa47ca620c04c684e3b210a6`,
reported no unresolved operations, and opened the existing schema-15 state.
Read-only inspection found one active Codex Workstream in
`recovery_required`, one corresponding `starting` Runtime with recorded PID and
birth, and one retained ProviderBinding on the prior Runtime generation. The
database hash was:

```text
2813e355f051e337b154eab741616917cec2ac6f5254e5702d4e392b1e03ccf2  ~/.local/state/wsnav/host.sqlite
```

The locked release was built and atomically installed to
`~/.local/bin/wsnav`. The release and installed executable are byte-identical:

```text
1bbf53aa5ca1a02930140cca1ad8358e8f9b0b632311bd18ceee82017c084fe1  target/release/wsnav
1bbf53aa5ca1a02930140cca1ad8358e8f9b0b632311bd18ceee82017c084fe1  ~/.local/bin/wsnav
```

The installed binary reports `wsnav 0.1.0`, opens the existing state with an
empty `operations` result, and leaves the schema, lifecycle counts, retained
generation mismatch, and database hash byte-identical. The already-open
Navigator deliberately remains the prior D21 process until the operator closes
and reopens that presentation; installation did not restart it or alter its
live provider.

## Unclaimed evidence

- Recover was not invoked against the ordinary state, so the currently live
  provider, retained binding, and recovery marker remain unchanged.
- No provider command, pane content, native lifecycle action, observer
  installation, trust change, ordinary tmux server, or provider input was
  exercised.
- No remote CI or alternate Rust/tmux compatibility run is claimed.
