# Workstream Navigator

Workstream Navigator (`wsnav`) is a thin terminal navigator for persistent
coding-agent workstreams on the machine where it is running. It adds
organization, attachment, status, and a few compound workstream actions
around the provider's native terminal UI.

> **D17 status:** implementation is in progress. The shell-first Navigator and
> brokered Codex/OpenCode promotion path are active and the repository gate
> passes; final operator-gated live acceptance remains open. See the
> [roadmap](docs/roadmap.md#d17---shell-first-managed-session-onboarding).

## Host-local by design

WSNav controls only the host on which it is executing. Codex or OpenCode
remains the place where the user plans, codes, selects models and agents,
resumes history, and uses native commands. WSNav does not replace that UI or
store its conversations.

To work on another machine, open an ordinary SSH terminal, tab, or window to
that machine and run `wsnav` there. Multi-host work therefore means separate
host-local WSNav windows, one per SSH-entered host. WSNav does not register SSH
hosts, open or manage SSH, poll remote state, issue remote actions, bridge a
remote shell, or present a combined cross-host catalog.

If the outer SSH connection drops, the disposable presentation may end or
detach, but the host's private Runtime, provider process, native session, and
completed output remain untouched. Reconnect to the host, run `wsnav` again,
and attach to the same Runtime.

## What it owns

- A host-local catalog of registered Git Project Locations and Workstreams.
- Project grouping by exact, credential-free Git-origin evidence on that
  host; it never groups records across hosts.
- Starting, switching, parking, exact resume, same-provider fork, archive,
  restore, and bounded lost-Runtime recovery.
- A contextual, read-only observer-readiness check for provider actions that
  require it.
- One private tmux server per live Runtime. WSNav never uses or changes the
  user's ordinary tmux server or configuration.

The provider pane remains a real native provider TUI. WSNav never writes
status or management traffic into it, captures prompts/responses/output, or
replaces completed provider results before the user acts. The presentation has
one right-hand surface: either a managed provider TUI or the provisional
account shell selected from Workstreams. It does not add a split utility shell
below a provider.

## The shell-first navigator

D17 has two direct pages. Page selection is process-local and is not persisted;
`Left` and `Right` do not cycle views.

| Page | Purpose and direct controls |
| --- | --- |
| **Workstreams** | Default page with one pinned **Shell** card plus active Workstreams grouped by Project. `Enter` opens the selected shell or managed session; on a managed Workstream, `n` starts a same-provider session at its exact Location, `f` forks, `p` parks, `x` archives, `a` acknowledges attention, `r` recovers an unresolved operation, and `?` opens page help. |
| **Archived** | Project-grouped archived Workstreams. `u` restores the selected Workstream and returns to Workstreams without launching or attaching a provider. |

`.` opens or closes Archived; `Esc` returns to Workstreams. Footer hints pack
into complete key/action pairs at the available width, and `?` shows a compact,
colored page-specific reference. Actions always resolve an exact Workstream ID
or the presentation-local shell singleton.

A fresh presentation starts with the Shell card selected and its account shell
already visible on the right; reconnecting a detached presentation preserves
its existing surface. The card is a stable two-line surface: `Shell`, then an
abbreviated cwd with every parent shortened and the leaf folder kept whole,
for example `~/c/workstream-navigator`. This presentation-local display
evidence is neither persisted nor launch authority. Use ordinary shell
commands to choose a directory, then run `codex` or `opencode`; the native
command owns provider and launch-option choice. Successful brokered launch
registers the detected Git worktree root and promotes that same card into the
managed Workstream while adding a fresh Shell card.

## Observer readiness

Observer setup is not a Hosts page, settings page, or manual normal-workflow
mode. Startup detects readiness read-only. If a requested Codex Start, Resume,
Fork, or recovery action needs an unready observer, WSNav captures that exact
intent and its revisions, then offers a contextual guide.

The guide asks for explicit consent before creating or updating one exact
WSNav-owned profile, opens the provider's native trust review without granting
trust, and resumes the captured intent only after exact readiness and revision
revalidation. Declining changes nothing. Foreign, modified, disabled,
ambiguous, or live-Runtime-blocked integration state fails closed while
existing Runtime attachment remains available. Exact profile removal is an
exceptional cleanup operation, not a setup option; it verifies ownership,
preserves foreign or modified state, and refuses while managed Runtimes are
live. A non-interactive CLI request returns bounded guidance to use
interactive `wsnav` rather than installing or reviewing a profile.

## The D16 state boundary

D16 is a clean break from the former client catalog. On an existing state root,
only an ordinary interactive launch may show the pre-presentation confirmation.
It names what is discarded and what is preserved; declining performs no
mutation. The exact legacy files are:

```text
client.sqlite
client.sqlite-wal
client.sqlite-shm
```

Those files are deleted without being opened, read, imported, renamed, or
backed up. D16 performs no importer, dual write, automatic backup, downgrade,
or rollback migration. An optional offline backup is an operator procedure:
park or stop managed Runtimes, exit WSNav, and create and verify a copy of the
complete state root before confirming cutover. Restoring that external copy is
the only downgrade path.

The preserved host state includes HostIdentity, integrations, ProjectLocations
and browser root, Workstream provider/activity/lifecycle fields, Runtime
generations, OpenCode handles, provider bindings, attention, compound
operations, private tmux servers, native provider sessions/history, and
completed output. Schema 12 is migrated transactionally to schema 13 using
`host.sqlite` only; fresh state is created directly at schema 13. Projects and
label-source Locations are rebuilt deterministically from current-host
ProjectLocations. Partial cleanup is retryable; a failed host migration leaves
schema 12 intact and blocks ordinary navigation until the confirmed
transition completes.

Before deletion, D16 proves ownership of any legacy presentation. An attached
client, utility shell, or native observer-review surface blocks mutation. The
launcher may offer a drain-only attachment that opens no host state, so the
operator can finish and quit that old presentation. Only one exact detached
ordinary presentation may then be retired under the transition lease. Runtime
tmux servers, provider processes, native sessions, and provider output are
never targeted by presentation retirement. Ambiguous or foreign artifacts fail
closed.

## Build, install, and CLI

WSNav is currently source-installed. The high-level local workflow is:

```console
cargo build --locked --release
install -m 755 target/release/wsnav ~/.local/bin/wsnav
wsnav
```

Development requires Rust 1.88 or newer, Python 3, Cargo Deny 0.20.x, Git,
`jq`, Ruff 0.16.x, and ShellCheck. Run the repository gate from the checkout:

```console
scripts/check
```

`wsnav --help` is the high-level reference for the installed CLI. Direct CLI
operations remain optional scripting, diagnostics, and break-glass parity;
ordinary work happens in the Navigator/provider presentation. D16 cutover is
an interactive startup transition, not a separate public command: only the
ordinary `wsnav` launch may present and accept its exact confirmation.

## See it

The [historical product captures](docs/media/README.md) show the pre-D16
two-pane baseline with privacy-safe fixture data. They are retained design
history, not current D16 UI or acceptance evidence.

## Documentation

- [Product and architecture design](docs/design.md)
- [Delivery roadmap and acceptance gates](docs/roadmap.md)
- [Documentation map and current operator contract](docs/README.md)
- [Product captures](docs/media/README.md)
- [Historical acceptance, spike, and study evidence](docs/evidence/README.md)

## License

[MIT](LICENSE)
