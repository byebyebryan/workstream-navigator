# Workstream Navigator

Workstream Navigator (`wsnav`) is a thin terminal navigator for persistent
coding-agent workstreams on the machine where it is running. It adds
organization, attachment, status, and a few compound workstream actions
around the provider's native terminal UI.

> **D19 status:** implemented and locally accepted for operator inspection.
> Checkpoint `a0ec38b` completes the tmux-derived navigation contract; the
> current focus-and-frame refinement passes the full repository gate and the Rust
> 1.88/tmux 3.3a matrix and is installed byte-identically at SHA-256
> `46365fe25fe0edacc728f4f1269487a24671a4ad90695264db2e70ed55e26b2c`.
> This is local/disposable acceptance, not remote-CI or live-provider
> acceptance. D18 checkpoint `c961c7e` remains the latest artifact with
> separately authorized Codex/OpenCode lifecycle evidence. See the
> [roadmap](docs/roadmap.md#completed-checkpoint-d19-tmux-derived-presentation-navigation).

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

The Navigator has two direct pages. Page selection is process-local and is not persisted;
`Left` and `Right` do not cycle views.

| Page | Purpose and direct controls |
| --- | --- |
| **Workstreams** | Default page with one pinned **Shell** card plus active Workstreams grouped by Project. `Enter` opens the selected shell or managed session; on a managed Workstream, `n` starts a same-provider session at its exact Location, `f` forks, `p` parks, `x` archives, `a` clears its unseen-result indicator, `r` recovers an unresolved operation, and `?` opens page help. |
| **Archived** | Project-grouped archived Workstreams. `u` restores the selected Workstream and returns to Workstreams without launching or attaching a provider. |

`.` opens or closes Archived; `Esc` returns to Workstreams. The compact footer
omits the baseline `↑↓` selection, `Enter` open/shell, and `a` acknowledge-result
hints; those remain in the complete `?` reference. Remaining footer hints pack
into complete key/action pairs at the available width. Actions always resolve
an exact Workstream ID or the presentation-local shell singleton.

Pane focus is tmux-owned and separate from Navigator row selection. Use
`Ctrl+b Left` and `Ctrl+b Right`, or deliberately press the primary mouse
button in a pane, to move keyboard control. `Enter` and every Navigator action
may replace the right-hand surface but do not move focus. While the managed
provider pane is focused, `Ctrl+b Up` and `Ctrl+b Down` attach the previous or
next eligible already-live Workstream in Navigator visual order; they skip
ineligible rows, never wrap, and never start, resume, recover, or otherwise
mutate a provider lifecycle. There is no separate pane-focus header: the
Navigator page title stays green while it receives keyboard input and dims to
dark gray while the provider pane is active.

One continuous green outline now wraps the entire Navigator, including its
footer. The adjacent tmux pane divider uses the same white foreground in both
focus states, with no forced background and no half-border indicators.

The presentation and each Runtime use closed private-tmux key tables. The
presentation retains detach, bounded help, literal `Ctrl+b`, Left/Right focus,
and provider-pane Up/Down switching. Direct Runtime attachment retains detach,
bounded help, literal `Ctrl+b`, and copy-mode entry. Split, window, layout,
menu, and arbitrary-command bindings are absent; none of these restrictions
changes the user's ordinary tmux server or configuration.

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
mode. Startup detects readiness read-only. Before a provisional shell can
reserve a Codex launch, its process-local wrapper asks for explicit consent and
opens native review; the original bounded argv remains only in shell memory and
is retried only after exact readiness. If an existing managed Codex Start,
Resume, Fork, or recovery action needs an unready observer, WSNav captures that
exact intent and its revisions, then offers the same contextual review in the
right pane.

The review path creates or updates one exact WSNav-owned profile only after
consent, opens the provider's native trust UI without granting trust, and
continues only after exact readiness and revision revalidation. Declining
changes nothing. Foreign, modified, disabled,
ambiguous, or live-Codex-Runtime-blocked integration state fails closed while
existing Runtime attachment remains available. Exact profile removal is an
exceptional cleanup operation, not a setup option; it verifies ownership,
preserves foreign or modified state, and refuses while managed Codex Runtimes
are live. A non-interactive CLI request returns bounded guidance to use
interactive `wsnav` rather than installing or reviewing a profile.

Its empty review cwd is owned by the exact presentation and removed only after
bounded process and filesystem identity checks. Cleanup is non-recursive;
interrupted cleanup is completed by presentation teardown after possible users
have stopped, while changed or non-empty paths are preserved.

## Current state boundary

The accepted implementation has one state epoch: schema 15. An absent state
root, or an exact private empty root, is created directly through the stable
`bootstrap.lock` protocol. An exact ready schema-15 root reopens normally, and
only unambiguous interrupted current-format bootstrap phases may resume.

Schemas 12 through 14, the retired client catalog, transition artifacts,
legacy presentation evidence, future schemas, and malformed or mixed roots are
refused before mutation. The implementation does not migrate, import, adopt,
drain, or partially clean an older WSNav root. Provider-owned native history
remains available through each provider's own tooling; old WSNav Runtime
ownership is not carried into the new epoch.

The authorized whole-product destructive reset parked the exact D17.1
Runtimes, removed its owned observer declaration, and quarantined the complete
schema-14 root as discarded state before installing and directly bootstrapping
schema 15. The exact quarantine was deleted after acceptance. There is no
migration, state rollback, automatic downgrade, or compatibility launcher.

## Build, install, and CLI

WSNav remains source-installed. This host runs the byte-identical D19 candidate
identified above; the D18 acceptance record separately identifies the exact
artifact used for live-provider release acceptance. Build and validate any
replacement before atomically installing its exact release artifact:

```console
cargo build --locked --release
scripts/check
```

Runtime prerequisites are Git, tmux, Bash or Zsh, and the util-linux `script`
command. Development additionally requires Rust 1.88 or newer, Python 3,
Cargo Deny 0.20.x, `jq`, Ruff 0.16.x, Ripgrep, and ShellCheck. Run the
repository gate from the checkout as shown above.

`wsnav --help` is the high-level reference for the installed CLI. Direct CLI
operations remain optional scripting, diagnostics, and break-glass parity;
ordinary work happens in the Navigator/provider presentation. The destructive
reset, exact-artifact installation, native observer trust, and disposable
live-provider release acceptance are complete for the accepted D18 artifact.
They are historical evidence, not accepted-release or live-provider evidence
for the locally installed D19 candidate.

## See it

The [historical product captures](docs/media/README.md) show a retired two-pane
baseline with privacy-safe fixture data. They are retained design history, not
current UI or acceptance evidence.

## Documentation

- [Product and architecture design](docs/design.md)
- [Delivery roadmap and acceptance gates](docs/roadmap.md)
- [Documentation map and current operator contract](docs/README.md)
- [Product captures](docs/media/README.md)
- [Historical acceptance, spike, and study evidence](docs/evidence/README.md)

## License

[MIT](LICENSE)
