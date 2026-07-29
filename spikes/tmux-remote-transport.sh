#!/usr/bin/env bash
# SPDX-License-Identifier: MIT
#
# An isolated transport spike for a nested local-tmux -> SSH -> remote-tmux path.
# It never uses the caller's tmux socket and never launches an agent provider.

set -euo pipefail

readonly STUDY="tmux-remote-transport"
readonly LOCAL_SESSION="wsnav-spike"
readonly REMOTE_SESSION="wsnav-spike"
readonly REMOTE_CODEX_SESSION="wsnav-codex-spike"
readonly NATIVE_PROMPT="Reply with the exact concatenation of WSNAV_NATIVE_ and RESULT, without spaces, and nothing else. Do not use tools, inspect files, or make changes."

host=""
result_path=""
debug_output_path=""
timeout_seconds=20
native_codex=false
local_root=""
local_socket=""
remote_root=""
remote_socket=""
remote_pane_pid=""
remote_codex_pane_pid=""
local_server_started=false
remote_server_started=false
cleanup_complete=true
dedicated_sockets=false
distinct_host=false
start_seconds=""
provider_id="none"
provider_version="not-applicable"
provider_contract_fingerprint="tmux-ssh-transport-v1"
native_provider_isolated=false
native_workspace_trust_confirmed=false
native_prompt_round_trip=false
native_process_persistence=false
native_result_tip_preserved=false

usage() {
    cat <<'EOF'
Usage: spikes/tmux-remote-transport.sh --host SSH_HOST [--native-codex] [--result PATH]
                                       [--debug-output PRIVATE_PATH]

Run the disposable local-tmux -> SSH -> remote-tmux transport study.

The script uses fresh tmux sockets under temporary directories, sends only
fixed probe messages to its own remote shell, and removes all temporary tmux
servers before producing a sanitized result. It never starts Codex or another
provider unless --native-codex is given. That mode launches one harmless
interactive Codex turn using a temporary CODEX_HOME and workspace. If --result
is given, the JSON result is written with mode 0600. --debug-output is an
explicit diagnostic escape hatch: it writes one raw local-pane capture with
mode 0600 and is never used by default.
EOF
}

die_usage() {
    printf 'error: %s\n' "$1" >&2
    usage >&2
    exit 64
}

while (($# > 0)); do
    case "$1" in
        --host)
            (($# >= 2)) || die_usage "--host requires an SSH host"
            host="$2"
            shift 2
            ;;
        --result)
            (($# >= 2)) || die_usage "--result requires a path"
            result_path="$2"
            shift 2
            ;;
        --debug-output)
            (($# >= 2)) || die_usage "--debug-output requires a path"
            debug_output_path="$2"
            shift 2
            ;;
        --timeout-seconds)
            (($# >= 2)) || die_usage "--timeout-seconds requires a value"
            timeout_seconds="$2"
            shift 2
            ;;
        --native-codex)
            native_codex=true
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            die_usage "unknown argument: $1"
            ;;
    esac
done

[[ -n "$host" ]] || die_usage "--host is required"
[[ "$timeout_seconds" =~ ^[1-9][0-9]*$ ]] || die_usage "timeout must be a positive integer"

for required_command in mktemp ssh tmux; do
    command -v "$required_command" >/dev/null 2>&1 || {
        printf 'error: required command is unavailable: %s\n' "$required_command" >&2
        exit 69
    }
done

umask 077

ssh_options=(
    -o BatchMode=yes
    -o ConnectTimeout=10
    -o ClearAllForwardings=yes
    -o LogLevel=ERROR
)

ssh_run() {
    # shellcheck disable=SC2029
    ssh "${ssh_options[@]}" "$host" "$@"
}

normal_local_tmux_fingerprint() {
    if env -u TMUX tmux list-sessions -F '#{session_name}:#{session_created}' >/dev/null 2>&1; then
        env -u TMUX tmux list-sessions -F '#{session_name}:#{session_created}' | sha256sum | awk '{print $1}'
    else
        printf 'absent\n'
    fi
}

normal_remote_tmux_fingerprint() {
    # shellcheck disable=SC2016
    ssh_run 'if env -u TMUX tmux list-sessions -F "#{session_name}:#{session_created}" >/dev/null 2>&1; then env -u TMUX tmux list-sessions -F "#{session_name}:#{session_created}" | sha256sum | awk "{print \$1}"; else printf "absent\\n"; fi'
}

remote_command() {
    local rendered_command
    printf -v rendered_command 'tmux -S %q %s' "$remote_socket" "$1"
    ssh_run "$rendered_command"
}

remote_display() {
    remote_display_for_session "$REMOTE_SESSION" "$1"
}

remote_display_for_session() {
    local session="$1"
    local format="$2"
    local rendered_command
    printf -v rendered_command 'tmux -S %q display-message -p -t %q %q' \
        "$remote_socket" \
        "${session}:0" \
        "$format"
    ssh_run "$rendered_command"
}

remote_has_session() {
    local session="$1"
    local rendered_command
    printf -v rendered_command 'tmux -S %q has-session -t %q' "$remote_socket" "$session"
    ssh_run "$rendered_command"
}

capture_local_pane() {
    tmux -S "$local_socket" capture-pane -p -t "${LOCAL_SESSION}:0.0" -S -200
}

capture_local_visible_pane() {
    tmux -S "$local_socket" capture-pane -p -t "${LOCAL_SESSION}:0.0"
}

wait_for_local_marker() {
    local marker="$1"
    local attempts=$((timeout_seconds * 5))
    local attempt

    for ((attempt = 0; attempt < attempts; attempt += 1)); do
        if [[ "$(capture_local_pane)" == *"$marker"* ]]; then
            return 0
        fi
        sleep 0.2
    done

    return 1
}

wait_for_remote_size() {
    local expected_size="$1"
    local attempts=$((timeout_seconds * 5))
    local attempt
    local observed_size

    for ((attempt = 0; attempt < attempts; attempt += 1)); do
        observed_size="$(remote_display '#{window_width}x#{window_height}')"
        if [[ "$observed_size" == "$expected_size" ]]; then
            return 0
        fi
        sleep 0.2
    done

    return 1
}

wait_for_visible_content() {
    local attempts=$((timeout_seconds * 5))
    local attempt
    local capture

    for ((attempt = 0; attempt < attempts; attempt += 1)); do
        capture="$(capture_local_visible_pane)"
        if [[ -n "${capture//[[:space:]]/}" ]]; then
            return 0
        fi
        sleep 0.2
    done

    return 1
}

wait_for_visible_marker() {
    local marker="$1"
    local attempts=$((timeout_seconds * 5))
    local attempt

    for ((attempt = 0; attempt < attempts; attempt += 1)); do
        if [[ "$(capture_local_visible_pane)" == *"$marker"* ]]; then
            return 0
        fi
        sleep 0.2
    done

    return 1
}

wait_for_visible_marker_to_disappear() {
    local marker="$1"
    local attempts=$((timeout_seconds * 5))
    local attempt

    for ((attempt = 0; attempt < attempts; attempt += 1)); do
        if [[ "$(capture_local_visible_pane)" != *"$marker"* ]]; then
            return 0
        fi
        sleep 0.2
    done

    return 1
}

wait_for_stable_visible_screen() {
    local attempts=$((timeout_seconds * 5))
    local attempt
    local previous=""
    local current
    local stable_samples=0

    for ((attempt = 0; attempt < attempts; attempt += 1)); do
        current="$(capture_local_visible_pane)"
        if [[ -n "${current//[[:space:]]/}" ]]; then
            if [[ "$current" == "$previous" ]]; then
                stable_samples=$((stable_samples + 1))
                if ((stable_samples >= 3)); then
                    return 0
                fi
            else
                previous="$current"
                stable_samples=0
            fi
        fi
        sleep 0.2
    done

    return 1
}

wait_for_stable_visible_result() {
    local marker="$1"
    local attempts=$((timeout_seconds * 5))
    local attempt
    local previous=""
    local current
    local stable_samples=0

    for ((attempt = 0; attempt < attempts; attempt += 1)); do
        current="$(capture_local_visible_pane)"
        if [[ "$current" == *"$marker"* ]]; then
            if [[ "$current" == "$previous" ]]; then
                stable_samples=$((stable_samples + 1))
                if ((stable_samples >= 3)); then
                    printf '%s' "$current"
                    return 0
                fi
            else
                previous="$current"
                stable_samples=0
            fi
        fi
        sleep 0.2
    done

    return 1
}

send_probe() {
    tmux -S "$local_socket" send-keys -t "${LOCAL_SESSION}:0.0" "$1" C-m
}

send_enter() {
    tmux -S "$local_socket" send-keys -t "${LOCAL_SESSION}:0.0" C-m
}

send_native_prompt() {
    tmux -S "$local_socket" send-keys -t "${LOCAL_SESSION}:0.0" "$NATIVE_PROMPT"
    sleep 0.2
    send_enter
}

cleanup() {
    set +e

    if [[ "$local_server_started" == true ]]; then
        tmux -S "$local_socket" kill-server >/dev/null 2>&1
        local_server_started=false
    fi

    if [[ "$remote_server_started" == true ]]; then
        remote_command 'kill-server' >/dev/null 2>&1
        remote_server_started=false
    fi

    if [[ -n "$remote_root" ]]; then
        local rendered_command
        printf -v rendered_command 'case %q in /tmp/wsnav-tmux-spike.*) rm -rf -- %q ;; *) exit 2 ;; esac' \
            "$remote_root" \
            "$remote_root"
        ssh_run "$rendered_command" >/dev/null 2>&1 || cleanup_complete=false
    fi

    if [[ -n "$local_root" ]]; then
        local removed=false
        local attempt
        for ((attempt = 0; attempt < 20; attempt += 1)); do
            rm -f -- "$local_socket"
            if rmdir "$local_root" >/dev/null 2>&1; then
                removed=true
                break
            fi
            sleep 0.1
        done
        [[ "$removed" == true ]] || cleanup_complete=false
    fi
}

capture_debug_output() {
    if [[ -z "$debug_output_path" ]]; then
        return
    fi

    install -m 600 /dev/null "$debug_output_path"
    capture_local_pane >"$debug_output_path" 2>&1 || true
}

emit_result() {
    local status="$1"
    local reason="$2"
    local provider_status="$3"
    local ordinary_tmux_unchanged="$4"
    local cleanup_status="$5"
    local elapsed_seconds
    local output

    elapsed_seconds=$(( $(date +%s) - start_seconds ))
    output="$(printf '{\n  "study": "%s",\n  "provider": {\n    "id": "%s",\n    "version": "%s",\n    "contract_fingerprint": "%s"\n  },\n  "status": "%s",\n  "reason": "%s",\n  "assertions": {\n    "dedicated_sockets": %s,\n    "distinct_host": %s,\n    "remote_persistence": %s,\n    "reconnect": %s,\n    "input_round_trip": %s,\n    "resize_propagates": %s,\n    "color_256": %s,\n    "mouse_capability_configured": %s,\n    "ordinary_tmux_unchanged": %s,\n    "native_provider_isolated": %s,\n    "native_workspace_trust_confirmed": %s,\n    "native_prompt_round_trip": %s,\n    "native_process_persistence": %s,\n    "native_result_tip_preserved": %s\n  },\n  "native_codex_status": "%s",\n  "cleanup": "%s",\n  "elapsed_seconds": %s\n}\n' \
        "$STUDY" \
        "$provider_id" \
        "$provider_version" \
        "$provider_contract_fingerprint" \
        "$status" \
        "$reason" \
        "$dedicated_sockets" \
        "$distinct_host" \
        "${remote_persistence:-false}" \
        "${reconnect:-false}" \
        "${input_round_trip:-false}" \
        "${resize_propagates:-false}" \
        "${color_256:-false}" \
        "${mouse_capability_configured:-false}" \
        "$ordinary_tmux_unchanged" \
        "$native_provider_isolated" \
        "$native_workspace_trust_confirmed" \
        "$native_prompt_round_trip" \
        "$native_process_persistence" \
        "$native_result_tip_preserved" \
        "$provider_status" \
        "$cleanup_status" \
        "$elapsed_seconds")"

    if [[ -n "$result_path" ]]; then
        install -m 600 /dev/null "$result_path"
        printf '%s' "$output" >"$result_path"
    else
        printf '%s' "$output"
    fi
}

finish() {
    local status="$1"
    local reason="$2"
    local provider_status="$3"
    local exit_code="$4"
    local local_after
    local remote_after
    local ordinary_tmux_unchanged=false
    local cleanup_status=complete

    capture_debug_output
    cleanup
    [[ "$cleanup_complete" == true ]] || cleanup_status=incomplete
    local_after="$(normal_local_tmux_fingerprint)"
    remote_after="$(normal_remote_tmux_fingerprint 2>/dev/null || printf unavailable)"
    if [[ "${local_before:-unavailable}" == "$local_after" && "${remote_before:-unavailable}" == "$remote_after" ]]; then
        ordinary_tmux_unchanged=true
    else
        cleanup_status=verification-failed
    fi

    if [[ "$status" == pass && ( "$cleanup_status" != complete || "$ordinary_tmux_unchanged" != true ) ]]; then
        status=falsified
        reason=cleanup-or-isolation-verification-failed
        exit_code=1
    fi

    trap - EXIT INT TERM
    emit_result "$status" "$reason" "$provider_status" "$ordinary_tmux_unchanged" "$cleanup_status"
    exit "$exit_code"
}

trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
start_seconds="$(date +%s)"
local_before="$(normal_local_tmux_fingerprint)"
remote_before="$(normal_remote_tmux_fingerprint 2>/dev/null)" || {
    emit_result blocked remote-ssh-unavailable blocked false not-run
    exit 2
}

local_hostname="$(hostname)"
remote_hostname="$(ssh_run hostname)" || finish blocked remote-hostname-unavailable blocked 2
if [[ "$local_hostname" == "$remote_hostname" ]]; then
    finish falsified same-host-not-a-remote-spike blocked 1
fi
distinct_host=true

remote_info="$(ssh_run 'bash -s' <<'REMOTE_SETUP'
set -euo pipefail
remote_root="$(mktemp -d /tmp/wsnav-tmux-spike.XXXXXX)"
chmod 700 "$remote_root"
remote_socket="$remote_root/tmux.sock"
remote_session="wsnav-spike"
endpoint_command='
printf "%s\\n" WSNAV_REMOTE_READY
while IFS= read -r request; do
    case "$request" in
        ping-*)
            printf "WSNAV_PONG:%s\\n" "$request"
            ;;
        color)
            printf "WSNAV_COLORS:%s\\n" "$(tput colors 2>/dev/null || printf 0)"
            ;;
        *)
            printf "%s\\n" WSNAV_UNKNOWN_REQUEST
            ;;
    esac
done
'
tmux -S "$remote_socket" -f /dev/null new-session -d -s "$remote_session" "$endpoint_command"
tmux -S "$remote_socket" set-option -g mouse on
tmux -S "$remote_socket" set-option -g status off
remote_pane_pid="$(tmux -S "$remote_socket" display-message -p -t "${remote_session}:0" "#{pane_pid}")"
printf "%s\\t%s\\t%s\\t%s\\n" "$remote_root" "$remote_socket" "$remote_session" "$remote_pane_pid"
REMOTE_SETUP
)" || finish blocked remote-tmux-unavailable blocked 2

IFS=$'\t' read -r remote_root remote_socket remote_session remote_pane_pid <<<"$remote_info"
[[ "$remote_session" == "$REMOTE_SESSION" ]] || finish falsified unexpected-remote-session-name blocked 1
remote_server_started=true

start_local_server() {
    local remote_session="$1"
    local attach_command
    printf -v attach_command 'exec ssh -tt -o BatchMode=yes -o ConnectTimeout=10 -o ClearAllForwardings=yes -o LogLevel=ERROR %q tmux -S %q attach-session -t %q' \
        "$host" \
        "$remote_socket" \
        "$remote_session"
    tmux -S "$local_socket" -f /dev/null new-session -d -s "$LOCAL_SESSION" -x 80 -y 24 "$attach_command"
    tmux -S "$local_socket" set-option -g mouse on
    tmux -S "$local_socket" set-option -g status off
    local_server_started=true
}

local_root="$(mktemp -d /tmp/wsnav-tmux-spike.XXXXXX)"
local_socket="$local_root/tmux.sock"
start_local_server "$REMOTE_SESSION" || finish falsified local-tmux-unavailable blocked 1
dedicated_sockets=true

wait_for_local_marker WSNAV_REMOTE_READY || finish falsified remote-ready-not-visible blocked 1

send_probe ping-one
wait_for_local_marker WSNAV_PONG:ping-one || finish falsified input-round-trip-failed blocked 1
input_round_trip=true

send_probe color
wait_for_local_marker WSNAV_COLORS: || finish falsified color-probe-not-visible blocked 1
color_count="$(capture_local_pane | awk -F: '/WSNAV_COLORS:/ { value=$2 } END { print value }')"
if [[ "$color_count" =~ ^[0-9]+$ && "$color_count" -ge 256 ]]; then
    color_256=true
else
    finish falsified fewer-than-256-colors blocked 1
fi

tmux -S "$local_socket" resize-window -t "${LOCAL_SESSION}:0" -x 101 -y 31
wait_for_remote_size 101x31 || finish falsified resize-did-not-propagate blocked 1
resize_propagates=true

if [[ "$(tmux -S "$local_socket" show-options -gv mouse)" == on && "$(remote_command 'show-options -gv mouse')" == on ]]; then
    mouse_capability_configured=true
else
    finish falsified mouse-capability-not-configured blocked 1
fi

before_reconnect_pid="$(remote_display '#{pane_pid}')"
tmux -S "$local_socket" kill-server
local_server_started=false
remote_has_session "$REMOTE_SESSION" >/dev/null || finish falsified remote-session-did-not-survive-detach blocked 1
after_detach_pid="$(remote_display '#{pane_pid}')"
if [[ "$before_reconnect_pid" != "$after_detach_pid" ]] || [[ "$before_reconnect_pid" != "$remote_pane_pid" ]]; then
    finish falsified remote-process-changed-on-detach blocked 1
fi
remote_persistence=true

start_local_server "$REMOTE_SESSION" || finish falsified local-reconnect-unavailable blocked 1
send_probe ping-two
wait_for_local_marker WSNAV_PONG:ping-two || finish falsified reconnect-input-round-trip-failed blocked 1
after_reconnect_pid="$(remote_display '#{pane_pid}')"
if [[ "$after_reconnect_pid" != "$before_reconnect_pid" ]]; then
    finish falsified remote-process-changed-on-reconnect blocked 1
fi
reconnect=true

if [[ "$native_codex" == false ]]; then
    if ssh_run 'command -v codex >/dev/null 2>&1'; then
        provider_status=not-run
    else
        provider_status=blocked
    fi
    finish pass transport-contract-proven "$provider_status" 0
fi

if ! ssh_run 'command -v codex >/dev/null 2>&1'; then
    finish blocked remote-codex-unavailable blocked 2
fi

provider_id=codex
provider_version="$(ssh_run 'codex --version' | awk 'NR == 1 { print $2 }' | tr -cd '[:alnum:].+-')"
[[ -n "$provider_version" ]] || provider_version=unknown
provider_contract_fingerprint="interactive-tui-remote-tmux-v1"

remote_codex_info="$(ssh_run bash -s -- "$remote_socket" "$REMOTE_CODEX_SESSION" "$remote_root" <<'REMOTE_CODEX_SETUP'
set -euo pipefail

socket="$1"
session="$2"
root="$3"
case "$root" in
    /tmp/wsnav-tmux-spike.*) ;;
    *) exit 2 ;;
esac

codex_root="$root/codex"
codex_home="$codex_root/home"
codex_workspace="$codex_root/workspace"
mkdir -p "$codex_home" "$codex_workspace"
chmod 700 "$codex_root" "$codex_home" "$codex_workspace"
install -m 600 "$HOME/.codex/auth.json" "$codex_home/auth.json"
CODEX_HOME="$codex_home" codex login status >/dev/null

printf -v command 'exec env CODEX_HOME=%q codex -s read-only -a never -C %q' \
    "$codex_home" \
    "$codex_workspace"
tmux -S "$socket" new-session -d -s "$session" -x 80 -y 24 "$command"
tmux -S "$socket" set-option -g mouse on
tmux -S "$socket" set-option -g status off
pane_pid="$(tmux -S "$socket" display-message -p -t "${session}:0" "#{pane_pid}")"
printf '%s\n' "$pane_pid"
REMOTE_CODEX_SETUP
)" || finish blocked isolated-codex-startup-failed blocked 2

remote_codex_pane_pid="$remote_codex_info"
native_provider_isolated=true

tmux -S "$local_socket" kill-server
local_server_started=false
start_local_server "$REMOTE_CODEX_SESSION" || finish falsified native-local-attach-unavailable falsified 1
wait_for_visible_content || finish falsified native-tui-not-visible falsified 1
wait_for_visible_marker 'Do you trust the contents of this directory?' || finish falsified native-workspace-trust-prompt-not-visible falsified 1
send_enter
wait_for_visible_marker_to_disappear 'Do you trust the contents of this directory?' || finish falsified native-workspace-trust-not-accepted falsified 1
native_workspace_trust_confirmed=true
wait_for_stable_visible_screen || finish falsified native-ready-screen-did-not-stabilize falsified 1

send_native_prompt
wait_for_local_marker WSNAV_NATIVE_RESULT || finish falsified native-result-not-visible falsified 1
native_result_capture="$(wait_for_stable_visible_result WSNAV_NATIVE_RESULT)" || finish falsified native-result-did-not-stabilize falsified 1
native_prompt_round_trip=true

native_before_detach_pid="$(remote_display_for_session "$REMOTE_CODEX_SESSION" '#{pane_pid}' | tr -d '\r')"
tmux -S "$local_socket" kill-server
local_server_started=false
remote_has_session "$REMOTE_CODEX_SESSION" >/dev/null || finish falsified native-session-did-not-survive-detach falsified 1
native_after_detach_pid="$(remote_display_for_session "$REMOTE_CODEX_SESSION" '#{pane_pid}' | tr -d '\r')"
if [[ "$native_before_detach_pid" != "$native_after_detach_pid" ]] || [[ "$native_before_detach_pid" != "$remote_codex_pane_pid" ]]; then
    finish falsified native-process-changed-on-detach falsified 1
fi
native_process_persistence=true

start_local_server "$REMOTE_CODEX_SESSION" || finish falsified native-reconnect-unavailable falsified 1
wait_for_local_marker WSNAV_NATIVE_RESULT || finish falsified native-result-not-visible-after-reconnect falsified 1
native_reconnected_capture="$(wait_for_stable_visible_result WSNAV_NATIVE_RESULT)" || finish falsified native-result-did-not-stabilize-after-reconnect falsified 1
if [[ "$native_reconnected_capture" != "$native_result_capture" ]]; then
    finish falsified native-result-tip-changed-on-reconnect falsified 1
fi
native_result_tip_preserved=true

finish pass native-codex-contract-proven pass 0
