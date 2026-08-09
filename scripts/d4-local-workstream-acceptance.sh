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
attach_socket=""

bounded_tmux_version() {
    local version
    version="$(env -u TMUX tmux -V 2>/dev/null || true)"
    version="$(LC_ALL=C printf '%s' "$version" | LC_ALL=C tr -cd '[:print:]' | head -c 64)"
    [[ -n "$version" ]] || version="unknown"
    printf '%s' "$version"
}

driver_pane_metadata() {
    driver_pane_dead="unknown"
    driver_exit_status="unknown"
    [[ -n "$attach_socket" ]] || return 0

    local pane_dead pane_exit_status
    pane_dead="$(env -u TMUX tmux -S "$attach_socket" display-message -p \
        -t "$attach_session:0.0" '#{pane_dead}' 2>/dev/null || true)"
    case "$pane_dead" in
        0|1) driver_pane_dead="$pane_dead" ;;
    esac
    pane_exit_status="$(env -u TMUX tmux -S "$attach_socket" display-message -p \
        -t "$attach_session:0.0" '#{pane_exit_status}' 2>/dev/null || true)"
    if [[ "$pane_exit_status" =~ ^[0-9]{1,4}$ ]]; then
        driver_exit_status="$pane_exit_status"
    fi
}

print_attachment_timeout() {
    local phase="$1"
    local attach_done_seen=0
    [[ -f "$attach_done" ]] && attach_done_seen=1
    driver_pane_metadata
    printf 'D4 %s timeout: tmux_version=%s client=%s native_marker=%s driver_marker=%s attach_done=%s driver_dead=%s driver_exit_status=%s\n' \
        "$phase" "$(bounded_tmux_version)" "$client_seen" "$native_marker_seen" \
        "$driver_marker_seen" "$attach_done_seen" "$driver_pane_dead" "$driver_exit_status" >&2
}

cleanup() {
    if [[ -n "$attach_socket" ]]; then
        env -u TMUX tmux -S "$attach_socket" kill-server >/dev/null 2>&1 || true
    fi
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

if [[ "${1:-}" == "--version" ]]; then
    printf 'codex fixture 0.1.0\n'
    exit 0
fi

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
printf 'WSNAV_FAKE_PROVIDER_NATIVE_SURFACE\n'
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
registration="$("$wsnav_bin" --state-root "$state_root" register --provider codex "$repository")"
source_workstream_id="${registration##* }"
"$wsnav_bin" --state-root "$state_root" start "$source_workstream_id"
sleep 1

source_status="$("$wsnav_bin" --state-root "$state_root" status "$source_workstream_id")"
grep -F 'lifecycle: Working' <<<"$source_status" >/dev/null
grep -F 'private runtime: live' <<<"$source_status" >/dev/null

runtime_socket="$(find "$state_root/run" -type s -name tmux.sock -print -quit)"
test -n "$runtime_socket"
runtime_session="$(env -u TMUX tmux -S "$runtime_socket" list-sessions -F '#{session_name}' | head -n 1)"
test -n "$runtime_session"
attach_socket="$task_root/attach.sock"
attach_session="wsnav-attach-driver"
attach_done="$task_root/attach.done"
printf -v attach_command '%q --state-root %q attach %q && touch %q' \
    "$wsnav_bin" "$state_root" "$source_workstream_id" "$attach_done"
env -u TMUX tmux -u -f /dev/null -S "$attach_socket" \
    new-session -d -s "$attach_session" -c "$repository" "$attach_command"

surface_seen=0
attached_client=""
client_seen=0
native_marker_seen=0
driver_marker_seen=0
for _ in {1..100}; do
    attached_clients="$(env -u TMUX tmux -S "$runtime_socket" list-clients -F '#{client_session}' 2>/dev/null || true)"
    if grep -Fx "$runtime_session" <<<"$attached_clients" >/dev/null 2>&1; then
        client_seen=1
        attached_client="$(env -u TMUX tmux -S "$runtime_socket" list-clients -F '#{client_tty}' | head -n 1)"
        test -n "$attached_client"
        native_surface="$(env -u TMUX tmux -S "$runtime_socket" capture-pane -p -J -t "$runtime_session:0.0" 2>/dev/null || true)"
        driver_surface="$(env -u TMUX tmux -S "$attach_socket" capture-pane -p -J -t "$attach_session:0.0" 2>/dev/null || true)"
        if grep -F 'WSNAV_FAKE_PROVIDER_NATIVE_SURFACE' <<<"$native_surface" >/dev/null; then
            native_marker_seen=1
        fi
        if grep -F 'WSNAV_FAKE_PROVIDER_NATIVE_SURFACE' <<<"$driver_surface" >/dev/null; then
            driver_marker_seen=1
        fi
        if [[ "$native_marker_seen" -eq 1 && "$driver_marker_seen" -eq 1 ]]; then
            surface_seen=1
            break
        fi
    fi
    sleep 0.1
done
if [[ "$surface_seen" -ne 1 ]]; then
    print_attachment_timeout surface
    exit 1
fi

env -u TMUX tmux -S "$runtime_socket" detach-client -t "$attached_client"
attach_completed=0
for _ in {1..100}; do
    if [[ -f "$attach_done" ]]; then
        attach_completed=1
        break
    fi
    sleep 0.1
done
if [[ "$attach_completed" -ne 1 ]]; then
    print_attachment_timeout completion
    exit 1
fi
attached_status="$("$wsnav_bin" --state-root "$state_root" status "$source_workstream_id")"
grep -F 'private runtime: live' <<<"$attached_status" >/dev/null

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
env -u TMUX tmux -S "$attach_socket" kill-server >/dev/null 2>&1 || true
attach_socket=""
destination_workstream_id=""
source_workstream_id=""
ordinary_tmux_after="$(env -u TMUX tmux list-sessions -F '#{session_name}:#{session_created}:#{session_windows}' -O name 2>/dev/null || true)"
[[ "$ordinary_tmux_before" == "$ordinary_tmux_after" ]]
printf 'D4 disposable local Workstream/fork acceptance passed\n'
