#!/usr/bin/env bash
# Disposable D4 Workstream/fork acceptance. It never uses normal Codex or tmux state.
set -euo pipefail

workspace_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
task_root="$(mktemp -d)"
ordinary_tmux_before="$(env -u TMUX tmux list-sessions -F '#{session_name}:#{session_created}:#{session_windows}' -O name 2>/dev/null || true)"
wsnav_bin=""
state_root=""
source_workstream_id=""
destination_workstream_id=""

cleanup() {
    if [[ -n "$wsnav_bin" && -n "$state_root" ]]; then
        [[ -z "$destination_workstream_id" ]] || "$wsnav_bin" --state-root "$state_root" park "$destination_workstream_id" >/dev/null 2>&1 || true
        [[ -z "$source_workstream_id" ]] || "$wsnav_bin" --state-root "$state_root" park "$source_workstream_id" >/dev/null 2>&1 || true
        "$wsnav_bin" --state-root "$state_root" remove-observer >/dev/null 2>&1 || true
    fi
    rm -rf -- "$task_root"
}
trap cleanup EXIT

cargo build --quiet
wsnav_bin="$workspace_root/target/debug/wsnav"
fake_bin="$task_root/bin"
state_root="$task_root/state"
codex_home="$task_root/codex-home"
repository="$task_root/repository"
fork_request="$task_root/fork-request.json"
mkdir -p "$fake_bin" "$codex_home" "$repository"

cat >"$fake_bin/codex" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail

if [[ "${1:-}" == "app-server" ]]; then
    while IFS= read -r line; do
        case "$line" in
            *'"id":1'*) printf '%s\n' '{"id":1,"result":{}}' ;;
            *'"id":2'*'thread/read'*)
                if [[ "$line" == *'destination-session'* ]]; then
                    printf '%s\n' '{"id":2,"result":{"thread":{"id":"destination-session","name":"source native name · fork"}}}'
                else
                    printf '%s\n' '{"id":2,"result":{"thread":{"id":"source-session","name":"source native name"}}}'
                fi
                ;;
            *'"id":2'*'thread/fork'*)
                printf '%s' "$line" >"$FAKE_CODEX_FORK_REQUEST"
                printf '%s\n' '{"id":2,"result":{"thread":{"id":"destination-session"}}}'
                ;;
            *'"id":2'*'thread/name/set'*) printf '%s\n' '{"id":2,"result":{}}' ;;
        esac
    done
    exit 0
fi

session_id="source-session"
source_value="startup"
if [[ " $* " == *" resume "* ]]; then
    session_id="destination-session"
    source_value="resume"
fi
printf '{"hook_event_name":"SessionStart","cwd":"%s","session_id":"%s","source":"%s"}' "$PWD" "$session_id" "$source_value" | wsnav --state-root "$FAKE_STATE_ROOT" _hook
if [[ "$session_id" == "source-session" ]]; then
    printf '{"hook_event_name":"UserPromptSubmit","cwd":"%s","session_id":"source-session","turn_id":"settled-turn"}' "$PWD" | wsnav --state-root "$FAKE_STATE_ROOT" _hook
    printf '{"hook_event_name":"Stop","cwd":"%s","session_id":"source-session","turn_id":"settled-turn"}' "$PWD" | wsnav --state-root "$FAKE_STATE_ROOT" _hook
    # This is the source's next in-progress turn. The D4 provider fork must
    # use only the preceding settled-turn boundary.
    printf '{"hook_event_name":"UserPromptSubmit","cwd":"%s","session_id":"source-session","turn_id":"running-turn"}' "$PWD" | wsnav --state-root "$FAKE_STATE_ROOT" _hook
fi
sleep 60
EOF
chmod 700 "$fake_bin/codex"

git -C "$repository" init -q -b main
git -C "$repository" config user.name wsnav-test
git -C "$repository" config user.email wsnav@example.test
printf 'base\n' >"$repository/committed.txt"
git -C "$repository" add committed.txt
git -C "$repository" commit -qm initial
printf 'source-only\n' >"$repository/source-only.txt"

export CODEX_HOME="$codex_home"
export FAKE_CODEX_FORK_REQUEST="$fork_request"
export FAKE_STATE_ROOT="$state_root"
export PATH="$fake_bin:$workspace_root/target/debug:$PATH"

"$wsnav_bin" --state-root "$state_root" setup --skip-review
profile_path="$codex_home/wsnav-observer.config.toml"
for hook in session_start user_prompt_submit stop session_end; do
    printf '\n[hooks.state."%s:%s:0:0"]\ntrusted_hash = "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"\n' \
        "$profile_path" "$hook" >>"$profile_path"
done
"$wsnav_bin" --state-root "$state_root" trust-observer
registration="$("$wsnav_bin" --state-root "$state_root" register "$repository")"
source_workstream_id="${registration##* }"
"$wsnav_bin" --state-root "$state_root" start "$source_workstream_id"
sleep 1

source_status="$("$wsnav_bin" --state-root "$state_root" status "$source_workstream_id")"
grep -F 'lifecycle: Working' <<<"$source_status" >/dev/null
grep -F 'private runtime: live' <<<"$source_status" >/dev/null

forked="$("$wsnav_bin" --state-root "$state_root" fork-workstream "$source_workstream_id")"
destination_workstream_id="${forked##* }"
sleep 1

grep -F '"lastTurnId":"settled-turn"' "$fork_request" >/dev/null
grep -F '"threadId":"source-session"' "$fork_request" >/dev/null
grep -F "\"cwd\":\"$repository\"" "$fork_request" >/dev/null
destination_status="$("$wsnav_bin" --state-root "$state_root" status "$destination_workstream_id")"
grep -F 'lifecycle: Idle' <<<"$destination_status" >/dev/null
grep -F 'private runtime: live' <<<"$destination_status" >/dev/null
source_status_after="$("$wsnav_bin" --state-root "$state_root" status "$source_workstream_id")"
grep -F 'lifecycle: Working' <<<"$source_status_after" >/dev/null

test -f "$repository/source-only.txt"
test ! -e "$state_root/worktrees"

"$wsnav_bin" --state-root "$state_root" park "$destination_workstream_id"
"$wsnav_bin" --state-root "$state_root" park "$source_workstream_id"
"$wsnav_bin" --state-root "$state_root" remove-observer
destination_workstream_id=""
source_workstream_id=""
ordinary_tmux_after="$(env -u TMUX tmux list-sessions -F '#{session_name}:#{session_created}:#{session_windows}' -O name 2>/dev/null || true)"
[[ "$ordinary_tmux_before" == "$ordinary_tmux_after" ]]
printf 'D4 disposable local Workstream/fork acceptance passed\n'
