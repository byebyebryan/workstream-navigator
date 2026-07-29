# Workstream Navigator

Workstream Navigator (`wsnav`) is a thin terminal layer for seeing and entering
persistent coding workstreams across machines while keeping each coding agent's
native terminal UI and workflow intact.

## Status

This repository is a clean-slate Rust reboot. It does not contain a usable
product yet, and it does not preserve compatibility with the earlier Python
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
also requires Git, jq, Ruff 0.16.0, and ShellCheck.

```console
scripts/check
cargo run -- --help
```

## Design

The proposed clean-slate V1 architecture is documented in [V1 design][]. It
keeps the native Codex workflow canonical, uses dedicated tmux runtimes and SSH
for attachment, and limits Workstream Navigator to hosts, project locations,
workstreams, status, and conservative worktree operations.

The approved D0-D5 implementation sequence and checkpoint acceptance gates are
tracked in the [V1 roadmap][].

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
[Spike 0001]: docs/spikes/0001-tmux-remote-transport.md
[Spike 0002]: docs/spikes/0002-codex-native-tui.md
[Spike 0004]: docs/spikes/0004-tmux-runtime-isolation.md
[Spike 0005]: docs/spikes/0005-codex-terminal-presentation.md
[Spike 0006]: docs/spikes/0006-codex-observer-profile.md
[Spike 0007]: docs/spikes/0007-codex-app-server-naming.md
[Spike 0008]: docs/spikes/0008-codex-running-settled-fork.md
[Study 0003]: docs/studies/0003-codex-app-server-runtime-boundary.md
[fixtures]: spikes/fixtures/
