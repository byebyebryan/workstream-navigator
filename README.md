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

The bootstrap intentionally has no third-party dependencies. Architecture and
runtime choices will follow a fresh design and validation pass.

```console
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
cargo run -- --help
```

## Design

The proposed clean-slate V1 architecture is documented in [V1 design][]. It
keeps the native Codex workflow canonical, uses dedicated tmux runtimes and SSH
for attachment, and limits Workstream Navigator to hosts, project locations,
workstreams, status, and conservative worktree operations.

## Decision studies

The isolated tmux/SSH transport and native Codex attach/detach/reconnect gates
passed. The App Server runtime-boundary study supports the dedicated-TUI plus
ephemeral-stdio split while leaving mutating contracts gated. See
[Spike 0001][], [Spike 0002][], [Spike 0004][], [Study 0003][], and the
sanitized [fixtures][].

[agent-switchboard-python-reference]: https://github.com/byebyebryan/agent-switchboard-python-reference
[V1 design]: docs/design.md
[Spike 0001]: docs/spikes/0001-tmux-remote-transport.md
[Spike 0002]: docs/spikes/0002-codex-native-tui.md
[Spike 0004]: docs/spikes/0004-tmux-runtime-isolation.md
[Study 0003]: docs/studies/0003-codex-app-server-runtime-boundary.md
[fixtures]: spikes/fixtures/
