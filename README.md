# Workstream Navigator

Workstream Navigator (`wsnav`) is a thin terminal layer for seeing and entering
persistent coding workstreams across machines while keeping each coding agent's
native terminal UI and workflow intact.

## Status

This repository is a clean-slate Rust reboot. D3 provides registered SSH host
control, cached multi-host navigation, and direct native terminal attachment;
its bounded local/remote Codex acceptance passed. D4 now has disposable
automated coverage for independent Workstreams and settled-prefix forks. Its
real native-Codex fork acceptance and V1 recovery closure remain roadmap work.
It does not preserve compatibility with the earlier Python prototype.

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

The approved D0-D5 implementation sequence and checkpoint acceptance gates are
tracked in the [V1 roadmap][].

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

The remote equivalents are `wsnav host new` and `wsnav host fork`, each with
the selected source revision. The normal navigator supplies that revision and
the opaque request key automatically. Fork is unavailable until a managed
source has an exact settled native turn; use `n` for an unrelated fresh start.

## SSH hosts

Install the same `wsnav` build yourself on the remote host at
`~/.local/bin/wsnav`, then register its existing SSH destination locally:

```console
wsnav register-remote snap
wsnav
```

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
results support the dedicated-TUI plus short-lived-stdio split and leave no
provider capability as a pre-implementation design blocker. See [Spike 0001][],
[Spike 0002][], [Spike 0004][], [Spike 0005][], [Spike 0006][], [Spike 0007][],
[Spike 0008][], [Study 0003][], and the sanitized [fixtures][].

[agent-switchboard-python-reference]: https://github.com/byebyebryan/agent-switchboard-python-reference
[V1 design]: docs/design.md
[V1 roadmap]: docs/roadmap.md
[D1 acceptance]: docs/acceptance-d1-local-codex.md
[D2 acceptance]: docs/acceptance-d2-local-navigator.md
[D3 acceptance]: docs/acceptance-d3-control-plane.md
[Spike 0001]: docs/spikes/0001-tmux-remote-transport.md
[Spike 0002]: docs/spikes/0002-codex-native-tui.md
[Spike 0004]: docs/spikes/0004-tmux-runtime-isolation.md
[Spike 0005]: docs/spikes/0005-codex-terminal-presentation.md
[Spike 0006]: docs/spikes/0006-codex-observer-profile.md
[Spike 0007]: docs/spikes/0007-codex-app-server-naming.md
[Spike 0008]: docs/spikes/0008-codex-running-settled-fork.md
[Study 0003]: docs/studies/0003-codex-app-server-runtime-boundary.md
[fixtures]: spikes/fixtures/
