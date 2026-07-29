#!/usr/bin/env bash
# SPDX-License-Identifier: MIT
#
# Disposable topology spike for one private tmux server per live Workstream.
# It never creates or mutates the caller's tmux server. It only makes a
# read-only before/after fingerprint and one scrubbed-environment visibility
# probe with TMUX unset.

set -euo pipefail

readonly STUDY="tmux-runtime-isolation"
readonly MAX_OVERHEAD_COUNT=32
# shellcheck disable=SC2016 # The endpoint must expand only inside its tmux pane.
readonly ENDPOINT_SCRIPT='\
set -eu
printf "WSNAV_RUNTIME_READY:%s:%s\\n" "$WSNAV_RUNTIME_LABEL" "$WSNAV_RUNTIME_MODE"
while IFS= read -r request; do
    case "$request" in
        ping)
            printf "WSNAV_PONG:%s\\n" "$WSNAV_RUNTIME_LABEL"
            ;;
        inner-scope)
            if [[ -n "${TMUX:-}" ]]; then
                observed="$(tmux list-sessions -F "#{session_name}" 2>/dev/null || true)"
                if [[ "$observed" == "$WSNAV_EXPECTED_SESSION" ]]; then
                    printf "WSNAV_INNER_PRIVATE_EXACT:%s\\n" "$WSNAV_RUNTIME_LABEL"
                else
                    printf "WSNAV_INNER_PRIVATE_UNEXPECTED:%s\\n" "$WSNAV_RUNTIME_LABEL"
                fi
            elif tmux list-sessions -F "#{session_name}" 2>/dev/null | grep -Fxq -- "$WSNAV_EXPECTED_SESSION"; then
                printf "WSNAV_INNER_SCRUBBED_LEAK:%s\\n" "$WSNAV_RUNTIME_LABEL"
            else
                printf "WSNAV_INNER_SCRUBBED_NO_LEAK:%s\\n" "$WSNAV_RUNTIME_LABEL"
            fi
            ;;
        *)
            printf "WSNAV_UNKNOWN_REQUEST:%s\\n" "$WSNAV_RUNTIME_LABEL"
            ;;
    esac
done
'

result_path=""
overhead_count=8
spike_root=""
cleanup_complete=true
dedicated_sockets=false
runtime_servers_distinct=false
one_session_per_server=false
one_window_per_session=false
one_pane_per_window=false
runtime_a_inner_scope=false
runtime_b_scrubbed_scope=false
sibling_survives=false
overhead_cohort_measured=false
overhead_total_rss_kib=0
start_seconds=""

declare -a runtime_labels=()
declare -a runtime_sockets=()
declare -a runtime_dirs=()

usage() {
    cat <<'EOF'
Usage: spikes/tmux-runtime-isolation.sh [--overhead-count COUNT] [--result PATH]

Run the disposable one-private-tmux-server-per-Workstream topology study.
It starts fixed shell endpoints only; it never launches Codex or another
provider. The caller's tmux server is read only for a before/after fingerprint.
EOF
}

die_usage() {
    printf 'error: %s\n' "$1" >&2
    usage >&2
    exit 64
}

while (($# > 0)); do
    case "$1" in
        --overhead-count)
            (($# >= 2)) || die_usage "--overhead-count requires a value"
            overhead_count="$2"
            shift 2
            ;;
        --result)
            (($# >= 2)) || die_usage "--result requires a path"
            result_path="$2"
            shift 2
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

[[ "$overhead_count" =~ ^[1-9][0-9]*$ ]] || die_usage "overhead count must be a positive integer"
((overhead_count <= MAX_OVERHEAD_COUNT)) || die_usage "overhead count must be at most $MAX_OVERHEAD_COUNT"

for required_command in awk grep install mktemp ps sha256sum tmux tr; do
    command -v "$required_command" >/dev/null 2>&1 || {
        printf 'error: required command is unavailable: %s\n' "$required_command" >&2
        exit 69
    }
done

umask 077

private_tmux() {
    local socket="$1"
    shift
    env -u TMUX tmux -S "$socket" "$@"
}

normal_tmux_fingerprint() {
    if env -u TMUX tmux list-sessions -F '#{session_name}:#{session_created}:#{session_windows}' -O name >/dev/null 2>&1; then
        env -u TMUX tmux list-sessions -F '#{session_name}:#{session_created}:#{session_windows}' -O name | sha256sum | awk '{print $1}'
    else
        printf 'absent\n'
    fi
}

runtime_session() {
    printf 'wsnav-%s\n' "$1"
}

runtime_socket_for() {
    printf '%s/%s/tmux.sock\n' "$spike_root" "$1"
}

runtime_dir_for() {
    printf '%s/%s\n' "$spike_root" "$1"
}

capture_runtime() {
    local socket="$1"
    local session="$2"
    private_tmux "$socket" capture-pane -p -t "${session}:0.0" -S -100 2>/dev/null
}

wait_for_marker() {
    local socket="$1"
    local session="$2"
    local marker="$3"
    local attempt

    for ((attempt = 0; attempt < 100; attempt += 1)); do
        if [[ "$(capture_runtime "$socket" "$session")" == *"$marker"* ]]; then
            return 0
        fi
        sleep 0.05
    done

    return 1
}

send_request() {
    local socket="$1"
    local session="$2"
    local request="$3"

    private_tmux "$socket" send-keys -t "${session}:0.0" "$request" C-m
}

line_count() {
    awk 'NF { count += 1 } END { print count + 0 }'
}

assert_runtime_structure() {
    local label="$1"
    local socket
    local session
    local sessions
    local windows
    local panes

    socket="$(runtime_socket_for "$label")"
    session="$(runtime_session "$label")"
    sessions="$(private_tmux "$socket" list-sessions -F '#{session_name}' 2>/dev/null || true)"
    windows="$(private_tmux "$socket" list-windows -t "$session" -F '#{window_id}' 2>/dev/null || true)"
    panes="$(private_tmux "$socket" list-panes -t "${session}:0" -F '#{pane_id}' 2>/dev/null || true)"

    [[ "$sessions" == "$session" ]] || return 1
    [[ "$(printf '%s\n' "$windows" | line_count)" == 1 ]] || return 1
    [[ "$(printf '%s\n' "$panes" | line_count)" == 1 ]] || return 1
}

runtime_server_pid() {
    local socket="$1"
    local session="$2"

    private_tmux "$socket" display-message -p -t "${session}:0" '#{pid}' 2>/dev/null | tr -d '\r'
}

create_runtime() {
    local label="$1"
    local mode="$2"
    local directory
    local socket
    local session
    local command

    directory="$(runtime_dir_for "$label")"
    socket="$(runtime_socket_for "$label")"
    session="$(runtime_session "$label")"
    mkdir -m 700 "$directory"

    case "$mode" in
        inherited)
            printf -v command 'exec env WSNAV_RUNTIME_LABEL=%q WSNAV_RUNTIME_MODE=%q WSNAV_EXPECTED_SESSION=%q bash -c %q' \
                "$label" \
                "$mode" \
                "$session" \
                "$ENDPOINT_SCRIPT"
            ;;
        scrubbed)
            printf -v command 'exec env -u TMUX WSNAV_RUNTIME_LABEL=%q WSNAV_RUNTIME_MODE=%q WSNAV_EXPECTED_SESSION=%q bash -c %q' \
                "$label" \
                "$mode" \
                "$session" \
                "$ENDPOINT_SCRIPT"
            ;;
        *)
            return 1
            ;;
    esac

    private_tmux "$socket" -f /dev/null new-session -d -s "$session" -x 101 -y 31 "$command"
    private_tmux "$socket" set-option -g status off
    private_tmux "$socket" set-option -g mouse on
    runtime_labels+=("$label")
    runtime_sockets+=("$socket")
    runtime_dirs+=("$directory")
}

collect_overhead_rss() {
    local start_index="$1"
    local index
    local socket
    local label
    local session
    local pid
    local rss
    local total=0

    for ((index = start_index; index < ${#runtime_labels[@]}; index += 1)); do
        label="${runtime_labels[index]}"
        socket="${runtime_sockets[index]}"
        session="$(runtime_session "$label")"
        pid="$(runtime_server_pid "$socket" "$session")"
        [[ "$pid" =~ ^[1-9][0-9]*$ ]] || return 1
        rss="$(ps -o rss= -p "$pid" | tr -d '[:space:]')"
        [[ "$rss" =~ ^[0-9]+$ ]] || return 1
        total=$((total + rss))
    done

    overhead_total_rss_kib="$total"
}

cleanup() {
    set +e

    local index
    local socket
    local directory
    local server_pid
    local attempt
    local server_stopped

    for ((index = 0; index < ${#runtime_sockets[@]}; index += 1)); do
        socket="${runtime_sockets[index]}"
        directory="${runtime_dirs[index]}"
        case "$directory" in
            "$spike_root"/*) ;;
            *)
                cleanup_complete=false
                continue
                ;;
        esac
        [[ -e "$directory" ]] || continue
        server_pid="$(runtime_server_pid "$socket" "$(runtime_session "${runtime_labels[index]}")" || true)"
        private_tmux "$socket" kill-server >/dev/null 2>&1 || true
        if [[ "$server_pid" =~ ^[1-9][0-9]*$ ]]; then
            server_stopped=false
            for ((attempt = 0; attempt < 20; attempt += 1)); do
                if ! ps -p "$server_pid" >/dev/null 2>&1; then
                    server_stopped=true
                    break
                fi
                sleep 0.05
            done
            [[ "$server_stopped" == true ]] || cleanup_complete=false
        fi
        rm -f -- "$socket"
        rmdir "$directory" >/dev/null 2>&1 || cleanup_complete=false
    done

    if [[ -n "$spike_root" ]]; then
        case "$spike_root" in
            /tmp/wsnav-tmux-runtime-spike.*)
                rmdir "$spike_root" >/dev/null 2>&1 || cleanup_complete=false
                ;;
            *)
                cleanup_complete=false
                ;;
        esac
    fi
}

emit_result() {
    local status="$1"
    local reason="$2"
    local ordinary_tmux_unchanged="$3"
    local cleanup_status="$4"
    local elapsed_seconds
    local output

    elapsed_seconds=$(( $(date +%s) - start_seconds ))
    output="$(printf '{\n  "study": "%s",\n  "provider": {\n    "id": "none",\n    "version": "not-applicable",\n    "contract_fingerprint": "tmux-per-runtime-server-v1"\n  },\n  "status": "%s",\n  "reason": "%s",\n  "assertions": {\n    "dedicated_sockets": %s,\n    "runtime_servers_distinct": %s,\n    "one_session_per_server": %s,\n    "one_window_per_session": %s,\n    "one_pane_per_window": %s,\n    "runtime_a_inner_scope": %s,\n    "runtime_b_scrubbed_scope": %s,\n    "sibling_survives": %s,\n    "overhead_cohort_measured": %s,\n    "ordinary_tmux_unchanged": %s\n  },\n  "metrics": {\n    "overhead_server_count": %s,\n    "overhead_total_rss_kib": %s\n  },\n  "cleanup": "%s",\n  "elapsed_seconds": %s\n}\n' \
        "$STUDY" \
        "$status" \
        "$reason" \
        "$dedicated_sockets" \
        "$runtime_servers_distinct" \
        "$one_session_per_server" \
        "$one_window_per_session" \
        "$one_pane_per_window" \
        "$runtime_a_inner_scope" \
        "$runtime_b_scrubbed_scope" \
        "$sibling_survives" \
        "$overhead_cohort_measured" \
        "$ordinary_tmux_unchanged" \
        "$overhead_count" \
        "$overhead_total_rss_kib" \
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
    local exit_code="$3"
    local local_after
    local ordinary_tmux_unchanged=false
    local cleanup_status=complete

    cleanup
    [[ "$cleanup_complete" == true ]] || cleanup_status=incomplete
    local_after="$(normal_tmux_fingerprint)"
    if [[ "${local_before:-unavailable}" == "$local_after" ]]; then
        ordinary_tmux_unchanged=true
    else
        cleanup_status=verification-failed
    fi

    if [[ "$status" == pass && ( "$cleanup_status" != complete || "$ordinary_tmux_unchanged" != true ) ]]; then
        status=falsified
        reason=cleanup-or-ordinary-tmux-verification-failed
        exit_code=1
    fi

    trap - EXIT INT TERM
    emit_result "$status" "$reason" "$ordinary_tmux_unchanged" "$cleanup_status"
    exit "$exit_code"
}

trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

start_seconds="$(date +%s)"
spike_root="$(mktemp -d /tmp/wsnav-tmux-runtime-spike.XXXXXX)"
chmod 700 "$spike_root"
local_before="$(normal_tmux_fingerprint)"

create_runtime runtime-a inherited || finish blocked runtime-a-startup-failed 2
create_runtime runtime-b scrubbed || finish blocked runtime-b-startup-failed 2
dedicated_sockets=true

runtime_a_socket="$(runtime_socket_for runtime-a)"
runtime_b_socket="$(runtime_socket_for runtime-b)"
runtime_a_session="$(runtime_session runtime-a)"
runtime_b_session="$(runtime_session runtime-b)"

wait_for_marker "$runtime_a_socket" "$runtime_a_session" 'WSNAV_RUNTIME_READY:runtime-a:inherited' || finish falsified runtime-a-not-ready 1
wait_for_marker "$runtime_b_socket" "$runtime_b_session" 'WSNAV_RUNTIME_READY:runtime-b:scrubbed' || finish falsified runtime-b-not-ready 1

runtime_a_pid="$(runtime_server_pid "$runtime_a_socket" "$runtime_a_session")"
runtime_b_pid="$(runtime_server_pid "$runtime_b_socket" "$runtime_b_session")"
if [[ ! "$runtime_a_pid" =~ ^[1-9][0-9]*$ || ! "$runtime_b_pid" =~ ^[1-9][0-9]*$ || "$runtime_a_pid" == "$runtime_b_pid" ]]; then
    finish falsified runtime-servers-not-distinct 1
fi
runtime_servers_distinct=true

for runtime_label in runtime-a runtime-b; do
    assert_runtime_structure "$runtime_label" || finish falsified runtime-structure-not-bounded 1
done
one_session_per_server=true
one_window_per_session=true
one_pane_per_window=true

send_request "$runtime_a_socket" "$runtime_a_session" ping
wait_for_marker "$runtime_a_socket" "$runtime_a_session" 'WSNAV_PONG:runtime-a' || finish falsified runtime-a-input-failed 1
send_request "$runtime_b_socket" "$runtime_b_session" ping
wait_for_marker "$runtime_b_socket" "$runtime_b_session" 'WSNAV_PONG:runtime-b' || finish falsified runtime-b-input-failed 1

send_request "$runtime_a_socket" "$runtime_a_session" inner-scope
wait_for_marker "$runtime_a_socket" "$runtime_a_session" 'WSNAV_INNER_PRIVATE_EXACT:runtime-a' || finish falsified inherited-tmux-scope-not-bounded 1
runtime_a_inner_scope=true

send_request "$runtime_b_socket" "$runtime_b_session" inner-scope
wait_for_marker "$runtime_b_socket" "$runtime_b_session" 'WSNAV_INNER_SCRUBBED_NO_LEAK:runtime-b' || finish falsified scrubbed-tmux-scope-leaked 1
runtime_b_scrubbed_scope=true

private_tmux "$runtime_a_socket" kill-server
rm -f -- "$runtime_a_socket"
rmdir "$(runtime_dir_for runtime-a)" || finish falsified runtime-a-cleanup-failed 1
private_tmux "$runtime_b_socket" has-session -t "$runtime_b_session" >/dev/null || finish falsified sibling-runtime-stopped 1
if [[ "$(runtime_server_pid "$runtime_b_socket" "$runtime_b_session")" != "$runtime_b_pid" ]]; then
    finish falsified sibling-runtime-restarted 1
fi
send_request "$runtime_b_socket" "$runtime_b_session" ping
wait_for_marker "$runtime_b_socket" "$runtime_b_session" 'WSNAV_PONG:runtime-b' || finish falsified sibling-runtime-input-failed 1
sibling_survives=true

overhead_start_index="${#runtime_labels[@]}"
for ((overhead_index = 1; overhead_index <= overhead_count; overhead_index += 1)); do
    create_runtime "overhead-$overhead_index" inherited || finish blocked overhead-runtime-startup-failed 2
    assert_runtime_structure "overhead-$overhead_index" || finish falsified overhead-runtime-structure-not-bounded 1
done
collect_overhead_rss "$overhead_start_index" || finish blocked overhead-rss-unavailable 2
overhead_cohort_measured=true

finish pass per-runtime-tmux-topology-proven 0
