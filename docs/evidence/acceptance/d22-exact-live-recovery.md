# D22 exact live recovery confirmation acceptance

Date: 2026-09-03

Status: corrected in checkpoint `8338a04`, locally accepted, byte-identically
installed, and accepted by an explicit operator re-test against the retained
live Codex recovery that falsified the first candidate. Checkpoint `ed74b0b`
passed its local/disposable gate and was installed for operator inspection, but
that inspection exposed its executable-name refusal. D21 checkpoint `868ee85`
is the starting boundary for this checkpoint. This record claims no remote-CI
or OpenCode acceptance.

## Accepted boundary

- Explicit Recover recognizes only a non-archived Codex Workstream in
  `recovery_required` whose exact retained ProviderBinding belongs to an older
  generation of the same `starting` Runtime.
- Before any durable mutation, recovery proves the exact private tmux topology,
  pane PID, process birth, cwd, an absolute executable ending in `codex` or
  Linux's exact `codex (deleted)` marker, and the complete generated
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

The corrected `scripts/check` passed on Rust 1.98.0 and tmux 3.7c. It ran strict
formatting and Clippy, 381 library tests, 10 presentation integration tests,
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

## Initial installed-artifact evidence

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
generation mismatch, and database hash byte-identical.

## Operator falsification

After the operator closed and reopened WSNav, explicit Recover still returned
bounded unavailable guidance and left the recovery marker intact. Read-only
inspection found that the exact private topology, pane PID, recorded process
birth, cwd, and full generated resume argv all matched. The sole rejected fact
was `/proc/<pid>/exe`: it reported `/usr/bin/codex (deleted)` because the Codex
binary had been replaced on disk after this process started.

The onboarding executable identity cannot authorize this recovery generation:
it correctly identifies an older provider generation and does not match the
live deleted executable. The correction instead accepts only Linux's exact
`codex (deleted)` kernel spelling alongside every existing D22 corroboration
fact. Relative names, alternate names, arbitrary suffixes, and doubled deleted
markers remain rejected.

## Corrected installed-artifact evidence

Checkpoint `8338a04` passed the complete repository gate with the added exact
executable-name test. Its locked release was atomically installed to
`~/.local/bin/wsnav`; the release and installed executable are byte-identical:

```text
30fd8bf0c6ac220b9c10088ddd623eec7dc301ffd77f1a5a4d4f36fccdfa5784  target/release/wsnav
30fd8bf0c6ac220b9c10088ddd623eec7dc301ffd77f1a5a4d4f36fccdfa5784  ~/.local/bin/wsnav
```

The corrected installed binary reports `wsnav 0.1.0`, reports no unresolved
operations, and leaves the schema-15 database byte-identical at
`2813e355f051e337b154eab741616917cec2ac6f5254e5702d4e392b1e03ccf2`.
The open Navigator still ran the preceding installed candidate until the
operator closed and reopened the presentation; installation itself restarted
no process.

## Corrected live operator acceptance

The operator closed and reopened WSNav after the corrected installation, then
selected the same `lazy-serializable` recovery card. The reopened Navigator's
executing image matched the corrected installed SHA-256, and explicit Recover
succeeded while the live provider executable continued to report Linux's exact
`/usr/bin/codex (deleted)` tombstone.

Sanitized state evidence before and after the successful action was:

| Fact | Before | After |
| --- | --- | --- |
| Workstream lifecycle / revision | `recovery_required` / 8 | `open` / 9 |
| Runtime lifecycle / revision | `starting` / 11 | `starting` / 11 |
| Binding generation / revision | retained prior generation / 3 | current generation / 4 |
| Unresolved operations | none | none |
| Schema | 15 | 15 |

The database changed only at the accepted reconciliation boundary, from
SHA-256
`2813e355f051e337b154eab741616917cec2ac6f5254e5702d4e392b1e03ccf2`
to
`d0ece9e3e2a65573b66638887036bc7082d5fe829d629b41424776f338af32c9`.
Read-only process inspection after success confirmed the same recorded Runtime
process remained live with the same private topology and deleted executable;
no provider restart, signal, input, pane capture, transcript access, or schema
migration occurred.

## Unclaimed evidence

- The first failed Recover attempt did not reconcile or otherwise mutate the
  live provider, retained binding, or recovery marker.
- Live acceptance covers only this exact Codex retained-session recovery; it
  does not transfer to initial/unbound binding, changed-session transition, or
  OpenCode recovery.
- No provider-TUI command/input, pane content, observer installation or trust
  change, ordinary tmux server, restart, or signal was exercised. The only
  provider-side operation was the designed bounded
  `thread/read(includeTurns=false)` corroboration.
- No remote CI or alternate Rust/tmux compatibility run is claimed.
