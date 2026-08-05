# Spike 0015: opencode provider feasibility

## Question

Can opencode (the agent running this repo's development sessions) serve as a
provider inside Workstream Navigator, which currently supports only Codex?
This study de-risks the two unknowns most likely to change the effort
estimate: fork semantics from a running source, and multiple concurrent
runtimes sharing one state database.

## Procedure and isolation

Each probe is a disposable Python harness (`spikes/opencode-*.py`) that runs
against opencode CLI `1.18.11` with a free model. Every harness creates its
own temporary project directory, isolated XDG roots, and sessions in its own
temporary OpenCode database. The harness copies the installed auth file into
that disposable data root with mode `0600`, then removes it with the rest of
the state. It inspects only the sessions it created, writes no files into the
user's projects, and never attaches to or terminates another opencode process
or any tmux server. Temporary directories and any spawned `opencode serve`
process are removed before the sanitized fixture is written.

Four probes were run:

1. **Running-source settled-prefix fork** (`opencode-running-settled-fork.py`):
   create a source with one settled turn, start an in-flight turn (a slow
   shell command) on that source, fork the running source, and compare the
   destination's messages with the source.
2. **CLI fork lineage** (`opencode-fork-lineage.py`): fork via
   `opencode run --fork` and check whether the destination is structurally
   linked to the source (SQLite `parent_id`, `GET /session/:id/children`).
3. **HTTP fork lineage** (`opencode-http-fork-lineage.py`): same check for
   `POST /session/:id/fork` on a headless `opencode serve` server.
4. **Shared-database concurrency** (`opencode-shared-db-concurrency.py`):
   launch four independent `opencode run` processes concurrently against the
   one probe-local SQLite database and check completion, `pragma integrity_check`,
   session visibility, and identity distinctness.

## Observed contract

### 1. Fork of a running source is settled-prefix-exact — pass

[Fixture][fork-exactness]: the fork contains the source's settled baseline
exactly and omits the in-flight turn's result, and is a distinct session. This
matches the semantics Workstream Navigator requires of its Fork Workstream
operation.

### 2. Fork lineage is not structural — known limitation

[CLI fixture][cli-lineage] and [HTTP fixture][http-lineage] agree: both fork
paths create a distinct destination session with the source conversation
copied in, but the destination's SQLite `parent_id` is `NULL` and the source's
`GET /session/:id/children` returns empty. Subagent sessions do populate
`parent_id`, so the children API itself works; fork sessions are simply not
linked. The only lineage marker is the destination title suffix `(fork #N)`,
which is presentation text and not identity.

The happy path is unaffected: the fork response carries the destination ID and
Workstream Navigator can record it immediately. The gap is confined to
recovering a fork after the response is lost, when the provider exposes no
structural way to find the destination from the source.

## Decision and limits

Fork-exactness and shared-database concurrency are validated; the fork-lineage
recovery gap is accepted as a known limitation, not a blocker. The later
[Spike 0016](0016-opencode-runtime-contract.md) adds the native Runtime and
observer evidence and fixes the recovery boundary: a lost Fork response is a
terminal `Failed` operation with `external_effect_unknown`. The source returns
to its pre-Fork visible state, WSNav does not create or adopt a destination,
and the user may inspect or clean up an unmanaged provider session in
opencode. WSNav never re-forks or guesses from title text; a new explicit Fork
is the only retry.

The result is opencode-`1.18.11`-specific. It does not authorize a production
provider adapter, transcript ingestion, a shared cross-Runtime server, or
weakening the existing provider contract. A future opencode release that
populates fork `parent_id` would remove the limitation without a design change.

[fixtures]: ../../../spikes/fixtures/
[fork-exactness]: ../../../spikes/fixtures/opencode-running-settled-fork.json
[cli-lineage]: ../../../spikes/fixtures/opencode-fork-lineage.json
[http-lineage]: ../../../spikes/fixtures/opencode-http-fork-lineage.json
[concurrency]: ../../../spikes/fixtures/opencode-shared-db-concurrency.json
