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
own temporary project directory and its own sessions under the shared opencode
state database. It inspects only the sessions it created, writes no files into
the user's projects, and never attaches to or terminates another opencode
process or any tmux server. Temporary directories and any spawned `opencode
serve` process are removed before the sanitized fixture is written.

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
   one global SQLite database and check completion, `pragma integrity_check`,
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
recovery gap is accepted as a known limitation, not a blocker. The accepted
degradation matches the existing `recovery_required` lifecycle: a lost fork
response marks the operation as requiring attention with an explicit
instruction to resolve the destination in opencode's native session list, and
never re-forks or guesses. Nothing is lost in the provider world because the
destination session is persisted by opencode; only Workstream Navigator's
bookkeeping is unresolved.

The result is opencode-`1.18.11`-specific. It does not authorize a production
provider adapter, transcript ingestion, a shared-server topology, or weakening
the existing provider contract. A future opencode release that populates fork
`parent_id` would remove the limitation without a design change.

[fixtures]: ../../../spikes/fixtures/
[fork-exactness]: ../../../spikes/fixtures/opencode-running-settled-fork.json
[cli-lineage]: ../../../spikes/fixtures/opencode-fork-lineage.json
[http-lineage]: ../../../spikes/fixtures/opencode-http-fork-lineage.json
[concurrency]: ../../../spikes/fixtures/opencode-shared-db-concurrency.json
