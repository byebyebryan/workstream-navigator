#!/usr/bin/env bash
# SPDX-License-Identifier: MIT
#
# Disposable local native-Codex presentation spike. It exercises a real Codex
# TUI in one private runtime tmux server and attaches it to a separate private
# two-pane presentation server. It never reads the caller's Codex config,
# hooks, sessions, or ordinary tmux server.

set -euo pipefail

readonly STUDY="codex-terminal-presentation"
readonly RUNTIME_SESSION="wsnav-runtime"
readonly PRESENTATION_SESSION="wsnav-presentation"
readonly NATIVE_RESULT_MARKER="WSNAV_TERMINAL_RESULT"
readonly NATIVE_PROMPT="Reply with the exact token WSNAV_TERMINAL_RESULT and nothing else. Do not use tools, inspect files, or make changes."
readonly PNG_BASE64="iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAusB9WlV6WQAAAAASUVORK5CYII="

result_path=""
debug_output_path=""
timeout_seconds=90
spike_root=""
runtime_socket=""
presentation_socket=""
runtime_config=""
presentation_config=""
runtime_server_pid=""
presentation_server_pid=""
runtime_provider_pane_pid=""
runtime_server_started=false
presentation_server_started=false
cleanup_complete=true
provider_id="codex"
provider_version="unknown"
contract_fingerprint="local-native-two-pane-retained-tmux-v2"
start_seconds=""
presentation_width=140
presentation_height=42

isolated_codex_home=false
private_runtime_unit=false
presentation_two_panes=false
direct_native_attach=false
native_tmux_retained=false
keyboard_submit=false
image_attachment_requested=false
resize_propagates=false
focus_round_trip=false
tmux_256color_configured=false
truecolor_environment_configured=false
mouse_capability_configured=false
native_process_survives_presentation_reconnect=false
native_result_tip_preserved=false
native_result_capture=""
native_reconnected_capture=""

usage() {
    cat <<'EOF'
Usage: spikes/codex-terminal-presentation.sh [--timeout-seconds SECONDS]
                                             [--result PATH]
                                             [--debug-output PRIVATE_PATH]

Run the isolated local native-Codex two-pane presentation study.

The study uses a temporary CODEX_HOME containing only a mode-0600 copy of the
existing auth cache and an empty disposable workspace. It launches one harmless
image-attached Codex prompt with read-only sandboxing, destroys and recreates
only its private presentation tmux server, and removes all temporary runtime
state before writing sanitized JSON.

--debug-output is an explicit diagnostic escape hatch. It writes one raw
provider-pane capture with mode 0600 and is never used by default.
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
        --debug-output)
            (($# >= 2)) || die_usage "--debug-output requires a path"
            debug_output_path="$2"
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

for required_command in awk base64 codex grep install mktemp ps sha256sum sleep tmux tr; do
    command -v "$required_command" >/dev/null 2>&1 || {
        printf 'error: required command is unavailable: %s\n' "$required_command" >&2
        exit 69
    }
done

[[ -f "$HOME/.codex/auth.json" ]] || {
    printf 'error: no readable Codex auth cache is available\n' >&2
    exit 69
}

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

line_count() {
    awk 'NF { count += 1 } END { print count + 0 }'
}

runtime_structure_is_bounded() {
    local sessions
    local windows
    local panes

    sessions="$(private_tmux "$runtime_socket" list-sessions -F '#{session_name}' 2>/dev/null || true)"
    windows="$(private_tmux "$runtime_socket" list-windows -t "$RUNTIME_SESSION" -F '#{window_id}' 2>/dev/null || true)"
    panes="$(private_tmux "$runtime_socket" list-panes -t "$RUNTIME_SESSION:0" -F '#{pane_id}' 2>/dev/null || true)"
    [[ "$sessions" == "$RUNTIME_SESSION" ]] &&
        [[ "$(printf '%s\n' "$windows" | line_count)" == 1 ]] &&
        [[ "$(printf '%s\n' "$panes" | line_count)" == 1 ]]
}

presentation_structure_is_bounded() {
    local sessions
    local windows
    local panes

    sessions="$(private_tmux "$presentation_socket" list-sessions -F '#{session_name}' 2>/dev/null || true)"
    windows="$(private_tmux "$presentation_socket" list-windows -t "$PRESENTATION_SESSION" -F '#{window_id}' 2>/dev/null || true)"
    panes="$(private_tmux "$presentation_socket" list-panes -t "$PRESENTATION_SESSION:0" -F '#{pane_id}' 2>/dev/null || true)"
    [[ "$sessions" == "$PRESENTATION_SESSION" ]] &&
        [[ "$(printf '%s\n' "$windows" | line_count)" == 1 ]] &&
        [[ "$(printf '%s\n' "$panes" | line_count)" == 2 ]]
}

capture_provider_pane() {
    private_tmux "$presentation_socket" capture-pane -p -t "$PRESENTATION_SESSION:0.1" -S -240 2>/dev/null
}

wait_for_provider_text() {
    local expected="$1"
    local attempts=$((timeout_seconds * 5))
    local attempt

    for ((attempt = 0; attempt < attempts; attempt += 1)); do
        if [[ "$(capture_provider_pane)" == *"$expected"* ]]; then
            return 0
        fi
        sleep 0.2
    done
    return 1
}

wait_for_provider_text_to_disappear() {
    local expected="$1"
    local attempts=$((timeout_seconds * 5))
    local attempt

    for ((attempt = 0; attempt < attempts; attempt += 1)); do
        if [[ "$(capture_provider_pane)" != *"$expected"* ]]; then
            return 0
        fi
        sleep 0.2
    done
    return 1
}

wait_for_stable_result() {
    local attempts=$((timeout_seconds * 5))
    local attempt
    local previous=""
    local current
    local stable_samples=0

    for ((attempt = 0; attempt < attempts; attempt += 1)); do
        current="$(capture_provider_pane)"
        if [[ "$current" == *"$NATIVE_RESULT_MARKER"* ]]; then
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

extract_result_tip() {
    local capture="$1"

    awk -v marker="$NATIVE_RESULT_MARKER" '
        index($0, marker) && length($0) <= length(marker) + 16 { print }
    ' <<<"$capture"
}

send_provider_text() {
    private_tmux "$presentation_socket" send-keys -t "$PRESENTATION_SESSION:0.1" "$1"
}

send_provider_enter() {
    private_tmux "$presentation_socket" send-keys -t "$PRESENTATION_SESSION:0.1" C-m
}

runtime_window_size() {
    private_tmux "$runtime_socket" display-message -p -t "$RUNTIME_SESSION:0" '#{window_width}x#{window_height}' 2>/dev/null | tr -d '\r'
}

wait_for_runtime_client() {
    local attempts=$((timeout_seconds * 5))
    local attempt
    local runtime_clients

    for ((attempt = 0; attempt < attempts; attempt += 1)); do
        runtime_clients="$(private_tmux "$runtime_socket" list-clients -F '#{client_pid}' 2>/dev/null || true)"
        if [[ "$(printf '%s\n' "$runtime_clients" | line_count)" == 1 ]]; then
            return 0
        fi
        sleep 0.2
    done
    return 1
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
    if [[ -n "$spike_root" ]]; then
        case "$spike_root" in
            /tmp/wsnav-codex-terminal-spike.*)
                rm -rf -- "$spike_root" || cleanup_complete=false
                ;;
            *)
                cleanup_complete=false
                ;;
        esac
    fi
}

capture_debug_output() {
    [[ -n "$debug_output_path" ]] || return 0
    install -m 600 /dev/null "$debug_output_path"
    {
        printf '%s\n' 'WSNAV_DEBUG_BEFORE_RECONNECT'
        printf '%s\n' "$native_result_capture"
        printf '%s\n' 'WSNAV_DEBUG_AFTER_RECONNECT'
        printf '%s\n' "$native_reconnected_capture"
    } >"$debug_output_path" 2>&1 || true
}

emit_result() {
    local status="$1"
    local reason="$2"
    local ordinary_tmux_unchanged="$3"
    local cleanup_status="$4"
    local elapsed_seconds
    local output

    elapsed_seconds=$(( $(date +%s) - start_seconds ))
    output="$(cat <<EOF
{
  "study": "$STUDY",
  "provider": {
    "id": "$provider_id",
    "version": "$provider_version",
    "contract_fingerprint": "$contract_fingerprint"
  },
  "status": "$status",
  "reason": "$reason",
  "assertions": {
    "isolated_codex_home": $isolated_codex_home,
    "private_runtime_unit": $private_runtime_unit,
    "presentation_two_panes": $presentation_two_panes,
    "direct_native_attach": $direct_native_attach,
    "native_tmux_retained": $native_tmux_retained,
    "keyboard_submit": $keyboard_submit,
    "image_attachment_requested": $image_attachment_requested,
    "resize_propagates": $resize_propagates,
    "focus_round_trip": $focus_round_trip,
    "tmux_256color_configured": $tmux_256color_configured,
    "truecolor_environment_configured": $truecolor_environment_configured,
    "mouse_capability_configured": $mouse_capability_configured,
    "native_process_survives_presentation_reconnect": $native_process_survives_presentation_reconnect,
    "native_result_tip_preserved": $native_result_tip_preserved,
    "ordinary_tmux_unchanged": $ordinary_tmux_unchanged
  },
  "cleanup": "$cleanup_status",
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
    local ordinary_tmux_unchanged=false
    local cleanup_status=complete
    local local_after

    capture_debug_output
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
set -g default-terminal "tmux-256color"
set -g status off
set -g mouse on
set-environment -g COLORTERM truecolor
EOF
    chmod 600 "$1"
}

start_runtime() {
    local codex_home="$spike_root/codex-home"
    local workspace="$spike_root/workspace"
    local image_path="$workspace/wsnav-pixel.png"
    local tmux_proof="$spike_root/runtime-tmux-proof"
    local runtime_command

    mkdir -m 700 "$codex_home" "$workspace"
    install -m 600 "$HOME/.codex/auth.json" "$codex_home/auth.json"
    printf '%s' "$PNG_BASE64" | base64 -d >"$image_path"
    chmod 600 "$image_path"
    isolated_codex_home=true
    image_attachment_requested=true

    printf -v runtime_command 'if ! env | grep -q "^TMUX="; then exit 86; fi; printf retained > %q; exec env CODEX_HOME=%q COLORTERM=truecolor codex -s read-only -a never -C %q -i %q' "$tmux_proof" "$codex_home" "$workspace" "$image_path"
    private_tmux "$runtime_socket" -f "$runtime_config" new-session -d -s "$RUNTIME_SESSION" -x 120 -y 42 "$runtime_command"
    runtime_server_started=true
    runtime_server_pid="$(private_tmux "$runtime_socket" display-message -p -t "$RUNTIME_SESSION:0" '#{pid}')"
    [[ "$runtime_server_pid" =~ ^[1-9][0-9]*$ ]] || return 1
    runtime_provider_pane_pid="$(private_tmux "$runtime_socket" display-message -p -t "$RUNTIME_SESSION:0.0" '#{pane_pid}')"
    [[ "$runtime_provider_pane_pid" =~ ^[1-9][0-9]*$ ]] || return 1
    runtime_structure_is_bounded || return 1
    [[ "$(private_tmux "$runtime_socket" show-options -gv default-terminal)" == tmux-256color ]] || return 1
    [[ "$(private_tmux "$runtime_socket" show-environment -g COLORTERM)" == COLORTERM=truecolor ]] || return 1
    [[ "$(private_tmux "$runtime_socket" show-options -gv mouse)" == on ]] || return 1
    tmux_256color_configured=true
    truecolor_environment_configured=true
    mouse_capability_configured=true
    private_runtime_unit=true
}

start_presentation() {
    local navigator_command
    local attach_command

    navigator_command='printf "WSNAV_NAVIGATOR_READY\n"; while IFS= read -r ignored; do :; done'
    printf -v attach_command 'exec env -u TMUX tmux -S %q attach-session -t %q' "$runtime_socket" "$RUNTIME_SESSION"
    private_tmux "$presentation_socket" -f "$presentation_config" new-session -d -s "$PRESENTATION_SESSION" -x "$presentation_width" -y "$presentation_height" "$navigator_command"
    private_tmux "$presentation_socket" split-window -h -d -t "$PRESENTATION_SESSION:0.0" -l 98 "$attach_command"
    presentation_server_started=true
    presentation_server_pid="$(private_tmux "$presentation_socket" display-message -p -t "$PRESENTATION_SESSION:0" '#{pid}')"
    [[ "$presentation_server_pid" =~ ^[1-9][0-9]*$ ]] || return 1
    presentation_structure_is_bounded || return 1
    [[ "$(private_tmux "$presentation_socket" show-options -gv default-terminal)" == tmux-256color ]] || return 1
    [[ "$(private_tmux "$presentation_socket" show-environment -g COLORTERM)" == COLORTERM=truecolor ]] || return 1
    [[ "$(private_tmux "$presentation_socket" show-options -gv mouse)" == on ]] || return 1
    wait_for_runtime_client || return 1
    presentation_two_panes=true
    direct_native_attach=true
}

trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

start_seconds="$(date +%s)"
ordinary_tmux_before="$(ordinary_tmux_fingerprint)"
provider_version="$(codex --version | awk 'NR == 1 { print $2 }' | tr -cd '[:alnum:].+-')"
[[ -n "$provider_version" ]] || provider_version=unknown

spike_root="$(mktemp -d /tmp/wsnav-codex-terminal-spike.XXXXXX)"
chmod 700 "$spike_root"
runtime_socket="$spike_root/runtime.sock"
presentation_socket="$spike_root/presentation.sock"
runtime_config="$spike_root/runtime.tmux.conf"
presentation_config="$spike_root/presentation.tmux.conf"
write_tmux_config "$runtime_config"
write_tmux_config "$presentation_config"

start_runtime || finish blocked isolated-runtime-startup-failed 2
start_presentation || finish falsified two-pane-presentation-startup-failed 1

wait_for_provider_text 'Do you trust the contents of this directory?' || finish falsified native-workspace-trust-prompt-not-visible 1
send_provider_enter
wait_for_provider_text_to_disappear 'Do you trust the contents of this directory?' || finish falsified native-workspace-trust-not-accepted 1

[[ "$(<"$spike_root/runtime-tmux-proof")" == retained ]] || finish falsified native-tmux-not-retained 1
native_tmux_retained=true

initial_runtime_size="$(runtime_window_size)"
private_tmux "$presentation_socket" resize-window -t "$PRESENTATION_SESSION:0" -x 156 -y 48
presentation_width=156
presentation_height=48
resized_runtime_size="$(runtime_window_size)"
if [[ "$initial_runtime_size" == "$resized_runtime_size" || ! "$resized_runtime_size" =~ ^[1-9][0-9]*x[1-9][0-9]*$ ]]; then
    finish falsified presentation-resize-did-not-reach-native-tui 1
fi
resize_propagates=true

private_tmux "$presentation_socket" select-pane -t "$PRESENTATION_SESSION:0.0"
private_tmux "$presentation_socket" select-pane -t "$PRESENTATION_SESSION:0.1"
focus_round_trip=true

send_provider_text "$NATIVE_PROMPT"
sleep 0.2
send_provider_enter
wait_for_provider_text "$NATIVE_RESULT_MARKER" || finish falsified native-result-not-visible 1
native_result_capture="$(wait_for_stable_result)" || finish falsified native-result-did-not-stabilize 1
native_result_tip="$(extract_result_tip "$native_result_capture")"
[[ "$(printf '%s\n' "$native_result_tip" | line_count)" == 1 ]] || finish falsified native-result-tip-not-isolatable 1
keyboard_submit=true

private_tmux "$presentation_socket" kill-server
presentation_server_started=false
server_stopped "$presentation_server_pid" || finish falsified presentation-server-did-not-stop 1
runtime_structure_is_bounded || finish falsified runtime-did-not-survive-presentation-detach 1
if [[ "$(private_tmux "$runtime_socket" display-message -p -t "$RUNTIME_SESSION:0.0" '#{pane_pid}')" != "$runtime_provider_pane_pid" ]]; then
    finish falsified native-process-changed-on-presentation-detach 1
fi

start_presentation || finish falsified presentation-reconnect-failed 1
if [[ "$(private_tmux "$runtime_socket" display-message -p -t "$RUNTIME_SESSION:0.0" '#{pane_pid}')" != "$runtime_provider_pane_pid" ]]; then
    finish falsified native-process-changed-on-presentation-reconnect 1
fi
native_process_survives_presentation_reconnect=true
wait_for_provider_text "$NATIVE_RESULT_MARKER" || finish falsified native-result-not-visible-after-reconnect 1
native_reconnected_capture="$(wait_for_stable_result)" || finish falsified native-result-did-not-stabilize-after-reconnect 1
native_reconnected_tip="$(extract_result_tip "$native_reconnected_capture")"
if [[ "$native_reconnected_tip" != "$native_result_tip" ]]; then
    finish falsified native-result-tip-changed-on-reconnect 1
fi
native_result_tip_preserved=true

finish pass native-two-pane-automation-proven 0
