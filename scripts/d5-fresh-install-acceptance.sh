#!/usr/bin/env bash
# Builds the packaged crate into a disposable user prefix and exercises only
# the owned observer lifecycle. It never touches ordinary Codex or tmux state.
set -euo pipefail

workspace_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
task_root="$(mktemp -d)"

cleanup() {
    rm -rf -- "$task_root"
}
trap cleanup EXIT

package_version="$(cargo metadata --no-deps --format-version=1 | jq -r '.packages[] | select(.name == "wsnav") | .version')"
[[ "$package_version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]
package_root="$workspace_root/target/package/wsnav-$package_version"
install_root="$task_root/install"
state_root="$task_root/state"
codex_home="$task_root/codex-home"

cargo package --locked --allow-dirty --quiet
test -f "$package_root/Cargo.toml"
cargo install --locked --path "$package_root" --root "$install_root" --quiet
wsnav_bin="$install_root/bin/wsnav"
test -x "$wsnav_bin"
"$wsnav_bin" --help >/dev/null

export CODEX_HOME="$codex_home"
"$wsnav_bin" --state-root "$state_root" setup --skip-review
doctor_output="$("$wsnav_bin" --state-root "$state_root" doctor)"
grep -F 'observer: TrustPending' <<<"$doctor_output" >/dev/null
profile_path="$codex_home/wsnav-observer.config.toml"
test -f "$profile_path"
"$wsnav_bin" --state-root "$state_root" remove-observer
test ! -e "$profile_path"
printf 'D5 disposable fresh-install acceptance passed\n'
