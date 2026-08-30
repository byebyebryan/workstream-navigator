#!/usr/bin/env bash
set -euo pipefail

# Deterministic current-contract coverage only. These tests allocate disposable
# roots and private tmux sockets; they never launch a provider or use ordinary
# WSNav state.
workspace_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
cd "${workspace_root}"

cargo test --locked --all-features presentation::
cargo test --locked --test presentation_recovery
cargo test --locked --all-features current_state_tests

printf 'current presentation and state acceptance passed\n'
