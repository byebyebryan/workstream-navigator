# Workstream Navigator documentation

## Status and authority

D16 host-local implementation, disposable repository gate, and separately
authorized live local and ordinary-SSH-entered-host acceptance are complete.
The current source also includes the documented post-acceptance rendering,
attachment, and Park convergence corrections and is installed locally for
operator inspection; those follow-ups do not rewrite the earlier candidate's
live-acceptance evidence.

- [Product and architecture design](design.md) is the V1 contract.
- [Delivery roadmap](roadmap.md) owns delivery order, checkpoint status, and
  exit gates.
- [Product captures](media/README.md) are frozen, privacy-safe pre-D16 design
  history and do not show the current reduced navigator.
- [Historical evidence](evidence/README.md) preserves the exact candidate,
  environment, procedure, and limitations of earlier checkpoints.

## Current operator contract

WSNav is host-local. Run it on the machine where the provider Runtime lives.
For another machine, establish ordinary SSH in a separate terminal, tab, or
window and run that host's own `wsnav`; separate hosts therefore have separate
WSNav windows. WSNav itself has no SSH registration, remote transport,
cross-host polling, remote attachment, or combined catalog. If an outer SSH
connection ends, the disposable presentation may be lost while the host-local
Runtime and provider remain untouched; reconnect and rerun `wsnav` on that host
to reattach.

The reduced navigator has three direct pages: Workstreams (active
Workstreams, Project-grouped), Projects (host-local Projects and Locations),
and Archived (Project-grouped restore). `,` opens Projects and `.` opens
Archived; `Esc` returns to Workstreams; `Left` and `Right` no longer cycle
views. Workstreams keeps `Enter`, `n`, `f`, `p`, `x`, `a`, and `?`; Projects
provides Location registration (`a`), browser-root configuration (`b`),
Location-based New (`n`), and explicit metadata refresh (`r`); Archived uses
`u` to restore without starting or attaching a provider. The installed
`wsnav --help` output is the CLI reference for that exact binary.

Projects registers exact host-local Git Locations through a bounded browser,
rooted at `~` by default. It uses relative cursors and does not expose raw
paths in snapshots or provider panes. Repository metadata is refreshed only by
the explicit Projects action; ordinary redraw, attachment, and fast
Workstream switching do not inspect Git. A selected Location can start a new
Workstream even when its Project has no active Workstreams.

Observer readiness is contextual rather than a page or manual setup mode.
Startup detects it read-only. A Codex action that needs readiness opens a guide
that captures the exact intent, asks explicit consent before one exact owned
profile is created or updated, opens native trust review without granting
trust, and continues only after readiness and revisions are revalidated.
Decline, foreign/modified/disabled/ambiguous state, or live-Runtime conflicts
leave state unchanged. Exact removal is an exceptional cleanup path; normal
CLI use returns guidance rather than installing or reviewing a profile.

D16 removes the client catalog in a clean break. The exact
`client.sqlite`, `client.sqlite-wal`, and `client.sqlite-shm` paths are
discarded without reading or importing their contents. There is no importer,
dual write, automatic backup, downgrade, or rollback adapter. An operator who
wants downgrade insurance may, before interactive confirmation, park or stop
managed Runtimes, exit WSNav, and create a verified offline copy of the
complete state root; restoring that external copy is outside D16 and is the
only downgrade procedure. Schema 12 migrates transactionally to schema 13
using `host.sqlite` only, preserving host/runtime/provider identity and history
while rebuilding host-local Projects.

Legacy presentation retirement is also fail-closed. Attached clients,
utility shells, and native observer-review surfaces block mutation. A bounded
drain-only attachment may be offered without opening host state so the user
can finish and quit the old presentation. Only an exact detached owned
presentation may be retired under the transition lease; Runtime tmux servers,
provider processes, sessions, and completed output are never targeted.

## Build and command references

The project is a source-installed operator beta. From a reviewed checkout,
build and install at a high level with `cargo build --locked --release` and
`install -m 755 target/release/wsnav ~/.local/bin/wsnav`; run `scripts/check`
for the repository gate. `wsnav --help` is the installed CLI reference. D16
cutover is available only through the ordinary interactive `wsnav` startup
flow; there is no public transition command. The normal workflow is the
Navigator beside the native provider TUI.
