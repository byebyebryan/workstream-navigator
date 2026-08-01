# Workstream Navigator

Workstream Navigator (`wsnav`) is a thin terminal layer for seeing and entering
persistent coding workstreams across machines while keeping each coding agent's
native terminal UI and workflow intact.

## Status

This repository is the clean-slate Rust implementation. D3 provides registered
SSH host control, cached multi-host navigation, and direct native terminal
attachment. D4 adds independent Workstreams and settled-prefix native-Codex
forks. D5 through D5.2 close the implemented V1 with runtime and operation
recovery, fresh-install/package verification, remote release diagnostics,
bounded control I/O, presentation correctness, and combined local/remote
native-Codex acceptance. D6 records the final [source-installed operator-beta
validation][D6 acceptance], and D6.1 adds accepted linked-worktree identity and
cross-host Project grouping polish, documented in the [D6.1 project-identity
acceptance][D6.1 acceptance]. D6.2 makes navigator controls discoverable
without adding another terminal surface, and D6.3 orders local and remote rows
by visible activity recency. D6.4 adds quiet host/Project grouping views, and
D6.5-D6.6 make visible Project accents distinct and readable. D7 broadens the
navigator-owned observer activation work into a complete terminal-first
workflow: the Workstreams home, Projects and Hosts management pages,
archive/restore, recovery, Project registration, and guarded SSH host
onboarding/offboarding are all available without leaving the navigator. The
Rust implementation does not preserve compatibility with the earlier Python
prototype.

The frozen prototype remains available as implementation evidence in
[agent-switchboard-python-reference][].

## Product boundaries

- Preserve the native agent TUI and native session workflow.
- Organize projects and workstreams without becoming a task manager.
- Treat local and remote hosts as first-class locations.
- Start with Codex and keep provider expansion possible.
- Add navigation, visibility, persistence, and low-friction workstream actions.

Workstream Navigator does not replace the provider UI, orchestrate autonomous
agent teams, transfer task context, or provide persistent project memory.

## Development

Development follows the latest stable Rust toolchain. The complete local gate
also requires Cargo Deny 0.20.2, Git, jq, Ruff 0.16.0, and ShellCheck.

```console
scripts/check
cargo run -- --help
```

## Design

The approved clean-slate V1 architecture is documented in [V1 design][]. It
keeps the native Codex workflow canonical, uses dedicated tmux runtimes and SSH
for attachment, and limits Workstream Navigator to hosts, project locations,
workstreams, status, and conservative worktree operations.

The approved implementation sequence through D7 and its checkpoint acceptance
gates are tracked in the [V1 roadmap][].

The accepted local Codex CLI slice and its sanitized native acceptance evidence
are documented in [D1 local native-Codex acceptance][D1 acceptance]. The run
required an operator's native Codex hook-trust decision and did not bypass that
review.

The accepted local two-pane navigator is documented in [D2 local navigator
acceptance][D2 acceptance]. It keeps the provider pane natively interactive
and uses a disposable private tmux presentation for navigation only.

The D3 control-plane implementation has local subprocess parity and fake-SSH
coverage. It deliberately does not install or update any remote executable.
Its recorded native-Codex acceptance used user-installed matching builds. See
[D3 control plane acceptance][D3 acceptance].

Independent and conversation-forked Workstreams are recorded in the [D4
acceptance][D4 acceptance]. Recovery and the final correctness hardening are
recorded in the [D5 acceptance][D5 acceptance], [D5.1 acceptance][D5.1
acceptance], and [D5.2 acceptance][D5.2 acceptance].

## Distribution

V1 is a source-installed operator beta at version `0.1.0`. It has no tagged
binary release, automatic updater, remote deployment service, or crates.io
publication. Build the reviewed checkout and install the executable explicitly:

```console
cargo build --locked --release
install -m 755 target/release/wsnav ~/.local/bin/wsnav
```

Use the same reviewed commit on every registered host. Before a stateful remote
action, verify the release, protocol, and schema contract:

```console
wsnav host doctor <alias>
```

The observer profile owns its exact hook executable path. On a fresh host,
install the final executable, then open `wsnav`; it creates the exact profile
and presents Codex's native hook review in the navigator's right pane. If an
existing development install moves from a build-tree symlink to the stable
installed path, first park every managed Workstream on that host, install the
new binary, then open a fresh `wsnav` and complete the native review again.
Profile updates deliberately refuse to run while a managed Runtime is live.

## First local project

The first `wsnav` on a host activates and reviews its observer; it does not
infer or register the current directory. After native approval, use the
Projects page (`,`) and `a` to add the Git checkout you want WSNav to manage.
The navigator asks for the host and checkout path without sending management
text to a provider pane:

```console
# approve the exact hook entries in the native right pane, then use Projects → a
wsnav
```

`wsnav register <path>` remains available for scripting or break-glass
diagnosis; it is not required for ordinary setup.

An empty navigator opens the same add flow rather than guessing a project from
the process working directory. Registration accepts a main
checkout, an externally created linked worktree, or a directory below either.
WSNav normalizes the selected checkout root separately from the repository's
primary worktree, so the selected Workstream keeps its own filesystem while
later managed Workstreams remain repository siblings.

When a registered checkout has one unambiguous network fetch remote, the
Projects page also shows its credential-free normalized label, such as
`↗ org/repo`. This label is display-only: WSNav groups
cross-host locations by its hidden fingerprint, never by the visible text.
Raw remote URLs, credentials, schemes, query strings, and filesystem paths are
not sent to the navigator.

The navigator footer stays compact during ordinary work. Press `?` for its
keyboard and mouse reference; this opens only inside the navigator pane, and
`?`, `Esc`, or `q` closes it. It never replaces or covers the native Codex pane.
Press `v` to cycle its local `Recent`, `By host`, and `By project` views; this
changes organization only, never the underlying Workstream or native session.

## Workstreams

Select a registered Workstream in the navigator and press `n` to start a fresh
managed Workstream from that project's recorded base, or press `f` to fork the
selected live Codex Workstream at its last completed turn. The source is not
interrupted; its current turn may continue while the destination opens in an
independent worktree. The direct equivalents are:

```console
wsnav new-workstream <source-workstream-id>
wsnav fork-workstream <source-workstream-id>
```

If a private runtime is conclusively gone, its row becomes `recovery required`.
Enter on that row (or `wsnav recover <workstream-id>`) opens native Codex resume:
an exact known thread resumes directly, while an unbound workstream uses
Codex's own resume picker. WSNav keeps the row in recovery until it observes a
corroborated native `resume`; it never replaces it with a blank thread.

The remote equivalents are `wsnav host new` and `wsnav host fork`, each with
the selected source revision. The normal navigator supplies that revision and
the opaque request key automatically. Fork is unavailable until a managed
source has an exact settled native turn; use `n` for an unrelated fresh start.

## SSH hosts

Install the same `wsnav` build yourself on the remote host at
`~/.local/bin/wsnav`. Then, in local WSNav, open Hosts (`.`), press `a`, and
enter its existing SSH destination (for example, `snap`). WSNav verifies and
registers the host, prepares its exact observer profile, and opens the remote
native Codex hook review in the right pane. Approve that profile in Codex.

```console
# in the local navigator: Hosts → a → snap → approve native review
wsnav
```

The Hosts page shows each host's active Project tree. `x` asks whether to
disconnect the local registration (keeping the remote observer) or offboard
the host (remove WSNav's exact observer, then disconnect). Offboarding refuses
while managed Runtimes are live or the host is unreachable.

When registered locations on different hosts have the same unambiguous
canonical fetch remote, WSNav presents them as one logical Project while
retaining separate host-owned Workstreams and checkouts. SSH and HTTPS
spellings of the same remote match. Repositories with missing, local-path, or
ambiguous remotes remain separate; WSNav never transmits the raw remote URL or
credentials.

Verify the remote's state-free release probe before using it:

```console
wsnav host doctor snap
```

If the probe is missing or reports an ABI, protocol, or host-schema mismatch,
update the remote manually from its checked-out source or release artifact, then
run the doctor command again. For a source checkout, the bounded V1 procedure
is:

```console
# on the remote host, in the trusted workstream-navigator checkout
git pull --ff-only
cargo build --locked --release
install -m 755 target/release/wsnav ~/.local/bin/wsnav

# on the local host
wsnav host doctor snap
```

WSNav never copies, bootstraps, or updates a remote binary itself. A failed
probe leaves cached remote rows visible but disables their actions until the
operator resolves the installation.

For a nonstandard SSH destination or executable path, keep the same short
command and supply only the difference:

```console
wsnav register-remote build --destination agent@build.example --executable /opt/wsnav/bin/wsnav
```

The navigator polls the registered host with bounded one-shot control calls
and keeps its last accepted state visible while the host is unavailable. Enter
on a remote row starts or resumes it when necessary, then opens a direct
`ssh -tt` provider attachment in the provider pane. `wsnav host reset snap`
is required after an intentional host-registry replacement or capability
change; Workstream Navigator never silently adopts it.

## Decision studies

The isolated tmux/SSH transport, native Codex presentation, scoped observer
profile, ephemeral naming, and running-source settled-fork gates passed. The
results support the implemented dedicated-TUI plus short-lived-stdio split and
its accepted provider contract. See [Spike 0001][], [Spike 0002][], [Spike
0004][], [Spike 0005][], [Spike 0006][], [Spike 0007][], [Spike 0008][], [Study
0003][], and the sanitized [fixtures][].

[agent-switchboard-python-reference]: https://github.com/byebyebryan/agent-switchboard-python-reference
[V1 design]: docs/design.md
[V1 roadmap]: docs/roadmap.md
[D1 acceptance]: docs/acceptance-d1-local-codex.md
[D2 acceptance]: docs/acceptance-d2-local-navigator.md
[D3 acceptance]: docs/acceptance-d3-control-plane.md
[D4 acceptance]: docs/acceptance-d4-workstreams.md
[D5 acceptance]: docs/acceptance-d5-v1-closure.md
[D5.1 acceptance]: docs/acceptance-d5.1-operational-closure.md
[D5.2 acceptance]: docs/acceptance-d5.2-correctness-closure.md
[D6 acceptance]: docs/acceptance-d6-operator-beta.md
[D6.1 acceptance]: docs/acceptance-d6.1-project-identity.md
[Spike 0001]: docs/spikes/0001-tmux-remote-transport.md
[Spike 0002]: docs/spikes/0002-codex-native-tui.md
[Spike 0004]: docs/spikes/0004-tmux-runtime-isolation.md
[Spike 0005]: docs/spikes/0005-codex-terminal-presentation.md
[Spike 0006]: docs/spikes/0006-codex-observer-profile.md
[Spike 0007]: docs/spikes/0007-codex-app-server-naming.md
[Spike 0008]: docs/spikes/0008-codex-running-settled-fork.md
[Study 0003]: docs/studies/0003-codex-app-server-runtime-boundary.md
[fixtures]: spikes/fixtures/
