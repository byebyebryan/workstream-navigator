#!/usr/bin/env bash
# SPDX-License-Identifier: MIT
#
# Disposable terminal-fidelity A/B study for the retained two-server
# presentation topology. It runs one deterministic streaming/typing workload
# inside a private runtime tmux server, captures the exact byte stream a client
# of the presentation tmux server would receive, and repeats the identical
# workload in a direct single-tmux server as a baseline. It never touches the
# caller's Codex config, hooks, sessions, or ordinary tmux server.

set -euo pipefail

readonly STUDY="codex-terminal-fidelity"
readonly RUNTIME_SESSION="wsnav-runtime"
readonly PRESENTATION_SESSION="wsnav-presentation"
readonly DIRECT_SESSION="wsnav-direct"
readonly WORKLOAD_DURATION_SECONDS=3
readonly CAPTURE_DURATION_SECONDS=4
readonly STARTUP_SETTLE_SECONDS=0.5

result_path=""
timeout_seconds=60
spike_root=""
runtime_socket=""
presentation_socket=""
direct_socket=""
runtime_config=""
presentation_config=""
direct_config=""
runtime_server_started=false
presentation_server_started=false
direct_server_started=false
cleanup_complete=true
contract_fingerprint="terminal-fidelity-synthetic-churn-v1"
start_seconds=""
presentation_width=140
presentation_height=42
runtime_width=120
runtime_height=40

ordinary_tmux_unchanged=false
cleanup_status=complete
nested_bytes=0
direct_bytes=0
nested_csi=0
direct_csi=0
nested_motion=0
direct_motion=0
nested_erase=0
direct_erase=0
nested_visibility=0
direct_visibility=0
nested_osc=0
direct_osc=0
bytes_ratio=0.0
csi_ratio=0.0
motion_ratio=0.0
erase_ratio=0.0
visibility_ratio=0.0
nested_motion_not_amplified=false
nested_bytes_not_amplified=false

usage() {
    cat <<'EOF'
Usage: spikes/codex-terminal-fidelity.sh [--timeout-seconds SECONDS]
                                         [--result PATH]

Run the isolated terminal-fidelity A/B study. It compares the byte stream a
presentation client receives in the retained nested topology against a direct
single-tmux baseline for the same deterministic workload. No Codex binary or
auth is required: the workload is a synthetic bounded streaming/typing emitter
whose cursor behavior is the unit under test.

--result writes a mode-0600 sanitized JSON fixture with aggregate counts and
amplication ratios only. No raw terminal content, paths, PIDs, or prompts are
committed.
EOF
}

die_usage() {
    printf 'error: %s\n' "$1" >&2
    usage >&2
    exit 64
}

while (($# > 0)); do
    case "$1" in
        --timeout-seconds)
            (($# >= 2)) || die_usage "--timeout-seconds requires a value"
            timeout_seconds="$2"
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

[[ "$timeout_seconds" =~ ^[1-9][0-9]*$ ]] || die_usage "timeout must be a positive integer"

for required_command in awk base64 grep install mktemp ps python3 script sha256sum sleep tmux; do
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

ordinary_tmux_fingerprint() {
    if env -u TMUX tmux list-sessions -F '#{session_name}:#{session_created}:#{session_windows}' -O name >/dev/null 2>&1; then
        env -u TMUX tmux list-sessions -F '#{session_name}:#{session_created}:#{session_windows}' -O name | sha256sum | awk '{print $1}'
    else
        printf 'absent\n'
    fi
}

server_stopped() {
    local pid="$1"
    local attempt

    [[ "$pid" =~ ^[1-9][0-9]*$ ]] || return 0
    for ((attempt = 0; attempt < 20; attempt += 1)); do
        if ! ps -p "$pid" >/dev/null 2>&1; then
            return 0
        fi
        sleep 0.05
    done
    return 1
}

cleanup() {
    set +e

    if [[ "$presentation_server_started" == true ]]; then
        private_tmux "$presentation_socket" kill-server >/dev/null 2>&1 || true
        server_stopped "$presentation_server_pid" || cleanup_complete=false
        presentation_server_started=false
    fi
    if [[ "$runtime_server_started" == true ]]; then
        private_tmux "$runtime_socket" kill-server >/dev/null 2>&1 || true
        server_stopped "$runtime_server_pid" || cleanup_complete=false
        runtime_server_started=false
    fi
    if [[ "$direct_server_started" == true ]]; then
        private_tmux "$direct_socket" kill-server >/dev/null 2>&1 || true
        server_stopped "$direct_server_pid" || cleanup_complete=false
        direct_server_started=false
    fi
    if [[ -n "$spike_root" ]]; then
        case "$spike_root" in
            /tmp/wsnav-codex-terminal-fidelity.*)
                rm -rf -- "$spike_root" || cleanup_complete=false
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
    local ordinary_tmux_unchanged_value="$3"
    local cleanup_status_value="$4"
    local elapsed_seconds
    local output

    elapsed_seconds=$(( $(date +%s) - start_seconds ))
    output="$(cat <<EOF
{
  "study": "$STUDY",
  "contract_fingerprint": "$contract_fingerprint",
  "status": "$status",
  "reason": "$reason",
  "assertions": {
    "nested_motion_not_amplified": $nested_motion_not_amplified,
    "nested_bytes_not_amplified": $nested_bytes_not_amplified,
    "ordinary_tmux_unchanged": $ordinary_tmux_unchanged_value
  },
  "metrics": {
    "nested_bytes": $nested_bytes,
    "direct_bytes": $direct_bytes,
    "bytes_ratio": $bytes_ratio,
    "nested_csi": $nested_csi,
    "direct_csi": $direct_csi,
    "csi_ratio": $csi_ratio,
    "nested_cursor_motion": $nested_motion,
    "direct_cursor_motion": $direct_motion,
    "cursor_motion_ratio": $motion_ratio,
    "nested_erase": $nested_erase,
    "direct_erase": $direct_erase,
    "erase_ratio": $erase_ratio,
    "nested_cursor_visibility": $nested_visibility,
    "direct_cursor_visibility": $direct_visibility,
    "cursor_visibility_ratio": $visibility_ratio,
    "nested_osc": $nested_osc,
    "direct_osc": $direct_osc
  },
  "cleanup": "$cleanup_status_value",
  "elapsed_seconds": $elapsed_seconds
}
EOF
)"

    if [[ -n "$result_path" ]]; then
        install -m 600 /dev/null "$result_path"
        printf '%s\n' "$output" >"$result_path"
    else
        printf '%s\n' "$output"
    fi
}

finish() {
    local status="$1"
    local reason="$2"
    local exit_code="$3"
    local local_after

    cleanup
    [[ "$cleanup_complete" == true ]] || cleanup_status=incomplete
    local_after="$(ordinary_tmux_fingerprint)"
    if [[ "$ordinary_tmux_before" == "$local_after" ]]; then
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

write_tmux_config() {
    cat >"$1" <<'EOF'
set -g status off
set -g mouse on
set -g default-terminal tmux-256color
set-environment -g COLORTERM truecolor
set -g extended-keys always
set -g extended-keys-format csi-u
set -as terminal-features ',xterm-ghostty:RGB:extkeys'
set -as terminal-features ',tmux-256color:RGB:extkeys'
EOF
    chmod 600 "$1"
}

write_workload() {
    cat >"$1" <<'EOF'
python3 -c '
import time, sys
out = sys.stdout
duration = float(__import__("os").environ.get("WSNAV_FIDELITY_DURATION", "3"))
out.write("\x1b[?1049h")
out.write("\x1b[?25l")
out.write("\x1b[2J\x1b[H")
frame = 0
end = time.monotonic() + duration
while time.monotonic() < end:
    frame += 1
    for i in range(8):
        out.write(f"\x1b[{1 + i};1H\x1b[Kline {frame} segment {i}")
    spinner = "|/-\\"[frame % 4]
    out.write(f"\x1b[9;1H\x1b[K{spinner} status {frame}")
    out.write("\x1b[10;1H\x1b[K")
    out.write("\x1b[?25h")
    out.write(f"\x1b[{9 + (frame % 2)};{1};H")
    out.flush()
    time.sleep(0.05)
out.write("\x1b[?25l\x1b[?1049l")
out.flush()
'
EOF
    chmod 700 "$1"
}

analyze_stream() {
    local stream="$1"
    python3 - "$stream" <<'EOF'
import re, sys

data = open(sys.argv[1], "rb").read()
csi = re.findall(rb"\x1b\[([0-9;?]*)([A-Za-z@`])", data)
motion = 0
erase = 0
for params, final in csi:
    if final in (b"H", b"f", b"A", b"B", b"C", b"D", b"G", b"d"):
        motion += 1
    if final in (b"J", b"K"):
        erase += 1
visibility = len(re.findall(rb"\x1b\[\?25[hl]", data))
osc = len(re.findall(rb"\x1b\]([0-9;]+)", data))
print(f"{len(data)} {len(csi)} {motion} {erase} {visibility} {osc}")
EOF
}

ratio() {
    python3 -c 'import sys; a=float(sys.argv[1]); b=float(sys.argv[2]); print(f"{a/b:.3f}" if b > 0 else "0.000")' "$1" "$2"
}

capture_presentation_stream() {
    script -q -e -c "env -u TMUX tmux -S $presentation_socket attach-session -t $PRESENTATION_SESSION" "$1" >/dev/null 2>&1 &
    local capture_pid=$!
    sleep "$CAPTURE_DURATION_SECONDS"
    kill "$capture_pid" >/dev/null 2>&1 || true
    wait "$capture_pid" >/dev/null 2>&1 || true
}

capture_direct_stream() {
    script -q -e -c "env -u TMUX tmux -S $direct_socket attach-session -t $DIRECT_SESSION" "$1" >/dev/null 2>&1 &
    local capture_pid=$!
    sleep "$CAPTURE_DURATION_SECONDS"
    kill "$capture_pid" >/dev/null 2>&1 || true
    wait "$capture_pid" >/dev/null 2>&1 || true
}

trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

start_seconds="$(date +%s)"
ordinary_tmux_before="$(ordinary_tmux_fingerprint)"

spike_root="$(mktemp -d /tmp/wsnav-codex-terminal-fidelity.XXXXXX)"
chmod 700 "$spike_root"
runtime_socket="$spike_root/runtime.sock"
presentation_socket="$spike_root/presentation.sock"
direct_socket="$spike_root/direct.sock"
runtime_config="$spike_root/runtime.tmux.conf"
presentation_config="$spike_root/presentation.tmux.conf"
direct_config="$spike_root/direct.tmux.conf"
write_tmux_config "$runtime_config"
write_tmux_config "$presentation_config"
write_tmux_config "$direct_config"

workload="$spike_root/churn.sh"
write_workload "$workload"

# Nested topology: runtime tmux runs the workload, presentation tmux attaches to it.
export WSNAV_FIDELITY_DURATION="$WORKLOAD_DURATION_SECONDS"
private_tmux "$runtime_socket" -f "$runtime_config" new-session -d -s "$RUNTIME_SESSION" -x "$runtime_width" -y "$runtime_height" "$workload"
runtime_server_started=true
runtime_server_pid="$(private_tmux "$runtime_socket" display-message -p -t "$RUNTIME_SESSION:0" '#{pid}')"
[[ "$runtime_server_pid" =~ ^[1-9][0-9]*$ ]] || finish blocked nested-runtime-startup-failed 2

private_tmux "$presentation_socket" -f "$presentation_config" new-session -d -s "$PRESENTATION_SESSION" -x "$presentation_width" -y "$presentation_height"
presentation_server_started=true
presentation_server_pid="$(private_tmux "$presentation_socket" display-message -p -t "$PRESENTATION_SESSION:0" '#{pid}')"
[[ "$presentation_server_pid" =~ ^[1-9][0-9]*$ ]] || finish blocked presentation-startup-failed 2
private_tmux "$presentation_socket" split-window -h -d -t "$PRESENTATION_SESSION:0.0" -l 108 "exec env -u TMUX tmux -S $runtime_socket attach-session -t $RUNTIME_SESSION"
sleep "$STARTUP_SETTLE_SECONDS"

nested_stream="$spike_root/nested.bin"
capture_presentation_stream "$nested_stream"

private_tmux "$presentation_socket" kill-server >/dev/null 2>&1 || true
presentation_server_started=false
private_tmux "$runtime_socket" kill-server >/dev/null 2>&1 || true
runtime_server_started=false
sleep 0.3

# Direct baseline: identical workload in a single tmux server.
private_tmux "$direct_socket" -f "$direct_config" new-session -d -s "$DIRECT_SESSION" -x "$runtime_width" -y "$runtime_height" "$workload"
direct_server_started=true
direct_server_pid="$(private_tmux "$direct_socket" display-message -p -t "$DIRECT_SESSION:0" '#{pid}')"
[[ "$direct_server_pid" =~ ^[1-9][0-9]*$ ]] || finish blocked direct-startup-failed 2
sleep "$STARTUP_SETTLE_SECONDS"

direct_stream="$spike_root/direct.bin"
capture_direct_stream "$direct_stream"

# Analyze both streams.
read -r nested_bytes nested_csi nested_motion nested_erase nested_visibility nested_osc <<<"$(analyze_stream "$nested_stream")"
read -r direct_bytes direct_csi direct_motion direct_erase direct_visibility direct_osc <<<"$(analyze_stream "$direct_stream")"

bytes_ratio="$(ratio "$nested_bytes" "$direct_bytes")"
csi_ratio="$(ratio "$nested_csi" "$direct_csi")"
motion_ratio="$(ratio "$nested_motion" "$direct_motion")"
erase_ratio="$(ratio "$nested_erase" "$direct_erase")"
visibility_ratio="$(ratio "$nested_visibility" "$direct_visibility")"

# The recorded defect: the nested presentation re-emits far more cursor
# positioning and erase sequences than the direct baseline for identical output.
# These are the objective bounds that a fidelity fix must bring under control.
if (( $(python3 -c "import sys; sys.exit(0 if float('$motion_ratio') <= 1.5 else 1)") )); then
    nested_motion_not_amplified=true
fi
if (( $(python3 -c "import sys; sys.exit(0 if float('$bytes_ratio') <= 1.3 else 1)") )); then
    nested_bytes_not_amplified=true
fi

if [[ "$nested_motion_not_amplified" == true && "$nested_bytes_not_amplified" == true ]]; then
    finish pass nested-presentation-emission-bounded 0
else
    finish falsified nested-presentation-cursor-emission-amplified 1
fi
