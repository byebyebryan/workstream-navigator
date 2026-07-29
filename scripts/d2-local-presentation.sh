#!/usr/bin/env bash
# SPDX-License-Identifier: MIT
#
# Disposable product-level smoke test for the D2 private presentation owner.
# It starts the real wsnav navigator in one private driver tmux server, proves
# that a separate private presentation server appears, sends only the native
# navigator quit key, and confirms both private servers clean up without
# touching the caller's ordinary tmux server. It launches no provider.

set -euo pipefail

binary_path="${1:-target/debug/wsnav}"
test_root=""
driver_socket=""
ordinary_before=""
ordinary_after=""

ordinary_tmux_fingerprint() {
    if env -u TMUX tmux list-sessions -F '#{session_name}:#{session_created}:#{session_windows}' -O name >/dev/null 2>&1; then
        env -u TMUX tmux list-sessions -F '#{session_name}:#{session_created}:#{session_windows}' -O name | sha256sum | awk '{print $1}'
    else
        printf 'absent\n'
    fi
}

cleanup() {
    if [[ -n "$driver_socket" ]]; then
        env -u TMUX tmux -S "$driver_socket" kill-server >/dev/null 2>&1 || true
    fi
    if [[ -n "$test_root" ]]; then
        rm -rf -- "$test_root"
    fi
}

die() {
    printf 'error: %s\n' "$1" >&2
    exit 1
}

trap cleanup EXIT

for command in awk git grep mktemp sha256sum tmux; do
    command -v "$command" >/dev/null 2>&1 || die "required command unavailable: $command"
done
[[ -x "$binary_path" ]] || die "wsnav binary is not executable: $binary_path"
binary_path="$(cd "$(dirname "$binary_path")" && pwd -P)/$(basename "$binary_path")"

test_root="$(mktemp -d /tmp/wd2.XXXXXX)"
driver_socket="$test_root/driver.sock"
ordinary_before="$(ordinary_tmux_fingerprint)"
repository="$test_root/alpha-project"

git init -q "$repository"
git -C "$repository" config user.name wsnav-d2
git -C "$repository" config user.email wsnav-d2@example.test
git -C "$repository" commit --allow-empty -m initial >/dev/null
"$binary_path" --state-root "$test_root/state" register "$repository" >/dev/null

env -u TMUX tmux -S "$driver_socket" new-session -d -s wsnav-d2-driver -x 160 -y 44 \
    "$binary_path" --state-root "$test_root/state"

presentation_socket=""
for _ in $(seq 1 100); do
    presentation_socket="$(find "$test_root/state/presentation" -name tmux.sock -type s -print -quit 2>/dev/null || true)"
    if [[ -n "$presentation_socket" ]] && env -u TMUX tmux -S "$presentation_socket" has-session >/dev/null 2>&1; then
        break
    fi
    sleep 0.05
done
[[ -n "$presentation_socket" ]] || die "private presentation server did not start"
rendered=false
for _ in $(seq 1 100); do
    if env -u TMUX tmux -S "$driver_socket" capture-pane -p -t wsnav-d2-driver:0 | grep -q 'alpha-project'; then
        rendered=true
        break
    fi
    sleep 0.05
done
[[ "$rendered" == true ]] || die "navigator did not render the registered Workstream"

env -u TMUX tmux -S "$driver_socket" send-keys -t wsnav-d2-driver:0 q

for _ in $(seq 1 100); do
    if ! env -u TMUX tmux -S "$driver_socket" has-session -t wsnav-d2-driver >/dev/null 2>&1; then
        break
    fi
    sleep 0.05
done
if env -u TMUX tmux -S "$driver_socket" has-session -t wsnav-d2-driver >/dev/null 2>&1; then
    die "navigator did not exit after its explicit quit action"
fi

if find "$test_root/state/presentation" -mindepth 1 -print -quit 2>/dev/null | grep -q .; then
    die "private presentation artifacts remain after navigator exit"
fi
ordinary_after="$(ordinary_tmux_fingerprint)"
[[ "$ordinary_before" == "$ordinary_after" ]] || die "ordinary tmux changed"

printf 'D2 disposable presentation acceptance passed\n'
