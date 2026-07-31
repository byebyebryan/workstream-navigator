#!/usr/bin/env bash
# Disposable D5.1 release-probe and output-boundary acceptance. It does not
# create host state, install hooks, launch a provider, or contact any tmux
# server.
set -euo pipefail

workspace_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
task_root="$(mktemp -d)"

cleanup() {
    rm -rf -- "$task_root"
}
trap cleanup EXIT

cargo build --quiet
wsnav_bin="$workspace_root/target/debug/wsnav"
state_root="$task_root/state"

probe="$("$wsnav_bin" --state-root "$state_root" _probe)"
jq -e '
  (.package_version | type == "string" and length > 0) and
  (.control_abi | type == "number" and . > 0) and
  (.protocol_version | type == "number" and . > 0) and
  (.host_schema_version | type == "number" and . > 0)
' <<<"$probe" >/dev/null
test ! -e "$state_root"

# All finite local control commands use the shared streaming runner. A direct
# Command::output call would buffer before applying a bound and is forbidden.
if rg -n '\.output\(\)' "$workspace_root/src" >/dev/null; then
    printf 'error: found an unbounded direct child-output call\n' >&2
    exit 1
fi

printf 'D5.1 disposable operational-closure acceptance passed\n'
