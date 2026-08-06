#!/usr/bin/env bash
# Disposable local D1 CLI acceptance. It never uses normal Codex or tmux state.
set -euo pipefail

workspace_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
task_root="$(mktemp -d)"
ordinary_tmux_before="$(env -u TMUX tmux list-sessions -F '#{session_name}:#{session_created}:#{session_windows}' -O name 2>/dev/null || true)"

cleanup() {
    rm -rf -- "$task_root"
}
trap cleanup EXIT

cargo build --quiet
wsnav_bin="$workspace_root/target/debug/wsnav"
fake_bin="$task_root/bin"
state_root="$task_root/state"
codex_home="$task_root/codex-home"
repository="$task_root/repository"
mkdir -p "$fake_bin" "$codex_home"

cat >"$fake_bin/codex" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail

if [[ "${1:-}" == "app-server" ]]; then
    while IFS= read -r line; do
        case "$line" in
            *'"id":1'*) printf '%s\n' '{"id":1,"result":{}}' ;;
            *'"id":2'*'thread/read'*) printf '%s\n' '{"id":2,"result":{"thread":{"id":"fake-session","name":""}}}' ;;
            *'"id":2'*'thread/name/set'*) printf '%s\n' '{"id":2,"result":{}}' ;;
        esac
    done
    exit 0
fi

source_value="startup"
if [[ " $* " == *" resume "* ]]; then
    source_value="resume"
fi
printf '{"hook_event_name":"SessionStart","cwd":"%s","session_id":"fake-session","source":"%s"}' "$PWD" "$source_value" | wsnav --state-root "$FAKE_STATE_ROOT" _hook
printf '{"hook_event_name":"UserPromptSubmit","cwd":"%s","session_id":"fake-session","turn_id":"fake-turn"}' "$PWD" | wsnav --state-root "$FAKE_STATE_ROOT" _hook
printf '{"hook_event_name":"Stop","cwd":"%s","session_id":"fake-session","turn_id":"fake-turn"}' "$PWD" | wsnav --state-root "$FAKE_STATE_ROOT" _hook
sleep 60
EOF
chmod 700 "$fake_bin/codex"

git init -q "$repository"
git -C "$repository" config user.name wsnav-test
git -C "$repository" config user.email wsnav@example.test
touch "$repository/README"
git -C "$repository" add README
git -C "$repository" commit -qm initial

export CODEX_HOME="$codex_home"
export FAKE_STATE_ROOT="$state_root"
export PATH="$fake_bin:$workspace_root/target/debug:$PATH"

"$wsnav_bin" --state-root "$state_root" setup --skip-review
profile_path="$codex_home/wsnav-observer.config.toml"
for hook in session_start user_prompt_submit stop session_end; do
    printf '\n[hooks.state."%s:%s:0:0"]\ntrusted_hash = "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"\n' \
        "$profile_path" "$hook" >>"$profile_path"
done
"$wsnav_bin" --state-root "$state_root" trust-observer
registration="$($wsnav_bin --state-root "$state_root" register --provider codex "$repository")"
workstream_id="${registration##* }"
"$wsnav_bin" --state-root "$state_root" start "$workstream_id"
sleep 1
status="$("$wsnav_bin" --state-root "$state_root" status "$workstream_id")"
grep -F 'lifecycle: Attention' <<<"$status" >/dev/null
grep -F 'private runtime: live' <<<"$status" >/dev/null
grep -F 'provider binding: bound' <<<"$status" >/dev/null
grep -F 'result attention: unseen' <<<"$status" >/dev/null
"$wsnav_bin" --state-root "$state_root" park "$workstream_id"
"$wsnav_bin" --state-root "$state_root" start "$workstream_id"
sleep 1
"$wsnav_bin" --state-root "$state_root" park "$workstream_id"
"$wsnav_bin" --state-root "$state_root" remove-observer

ordinary_tmux_after="$(env -u TMUX tmux list-sessions -F '#{session_name}:#{session_created}:#{session_windows}' -O name 2>/dev/null || true)"
[[ "$ordinary_tmux_before" == "$ordinary_tmux_after" ]]
printf 'D1 disposable local acceptance passed\n'
