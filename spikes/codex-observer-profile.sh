#!/usr/bin/env bash
# SPDX-License-Identifier: MIT
#
# Isolated Codex observer-profile and hook-authority spike. It uses a temporary
# CODEX_HOME, workspace, hook handler, state root, and private tmux sockets. It
# never reads the user's Codex configuration or trust store and never writes to
# the ordinary tmux server.

set -euo pipefail

readonly STUDY="codex-observer-profile"
readonly RUNTIME_SESSION="wsnav-observer-runtime"
readonly PRESENTATION_SESSION="wsnav-observer-presentation"
readonly PROFILE_NAME="wsnav-observer"
readonly RESULT_MARKER="WSNAV_OBSERVER_RESULT"
readonly RESULT_PROMPT="Reply with the exact token WSNAV_OBSERVER_RESULT and nothing else. Do not use tools, inspect files, or make changes."

result_path=""
timeout_seconds=90
spike_root=""
codex_home=""
workspace=""
runtime_socket=""
presentation_socket=""
tmux_config=""
hook_handler=""
event_log=""
seen_log=""
provider_pid_file=""
expected_cwd_file=""
expected_generation_file=""
observed_session_file=""
runtime_server_started=false
presentation_server_started=false
cleanup_complete=true
start_seconds=""
ordinary_tmux_before=""
provider_version="unknown"
contract_fingerprint="codex-profile-hook-authority-v1"

profile_layers_over_base=false
base_config_preserved=false
native_hook_trust_confirmed=false
promptless_trust_review_confirmed=false
promptless_review_left_native_session=false
session_start_observed=false
user_prompt_submit_observed=false
stop_observed=false
session_end_observed=false
lifecycle_order_confirmed=false
clear_rebind_observed=false
ordinary_launch_unobserved=false
trusted_profile_reused=false
large_unmanaged_payload_drained=false
missing_authority_rejected=false
stale_generation_rejected=false
forged_process_rejected=false
profile_collision_refused=false
modified_profile_removal_refused=false
exact_profile_removal_succeeds=false

usage() {
    cat <<'EOF'
Usage: spikes/codex-observer-profile.sh [--timeout-seconds SECONDS]
                                         [--result PATH]

Run the isolated Codex observer-profile and hook-authority study.

The study creates a temporary Codex home containing only the caller's auth
cache, a synthetic base config, and the spike-owned wsnav-observer profile. It
uses native workspace and hook trust prompts, starts no ordinary Codex session,
and removes all temporary state before emitting sanitized JSON.
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

for required_command in awk codex env git grep install jq mktemp ps sha256sum sleep tmux tr; do
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
        env -u TMUX tmux list-sessions -F '#{session_name}:#{session_created}:#{session_windows}' -O name |
            sha256sum |
            awk '{print $1}'
    else
        printf 'absent\n'
    fi
}

server_stopped() {
    local pid="$1"
    local attempt

    [[ "$pid" =~ ^[1-9][0-9]*$ ]] || return 0
    for ((attempt = 0; attempt < 40; attempt += 1)); do
        if ! ps -p "$pid" >/dev/null 2>&1; then
            return 0
        fi
        sleep 0.05
    done
    return 1
}

cleanup_runtime() {
    local runtime_pid=""
    local presentation_pid=""

    set +e
    if [[ "$presentation_server_started" == true ]]; then
        presentation_pid="$(private_tmux "$presentation_socket" display-message -p -t "$PRESENTATION_SESSION:0" '#{pid}' 2>/dev/null || true)"
        private_tmux "$presentation_socket" kill-server >/dev/null 2>&1 || true
        server_stopped "$presentation_pid" || cleanup_complete=false
        presentation_server_started=false
    fi
    if [[ "$runtime_server_started" == true ]]; then
        runtime_pid="$(private_tmux "$runtime_socket" display-message -p -t "$RUNTIME_SESSION:0" '#{pid}' 2>/dev/null || true)"
        private_tmux "$runtime_socket" kill-server >/dev/null 2>&1 || true
        server_stopped "$runtime_pid" || cleanup_complete=false
        runtime_server_started=false
    fi
    set -e
}

cleanup() {
    set +e
    cleanup_runtime
    if [[ -n "$spike_root" ]]; then
        case "$spike_root" in
            /tmp/wsnav-codex-observer-spike.*)
                rm -rf -- "$spike_root" || cleanup_complete=false
                ;;
            *)
                cleanup_complete=false
                ;;
        esac
    fi
}

capture_provider() {
    private_tmux "$presentation_socket" capture-pane -p -t "$PRESENTATION_SESSION:0.0" -S -160 2>/dev/null
}

wait_for_text() {
    local expected="$1"
    local attempts=$((timeout_seconds * 5))
    local attempt

    for ((attempt = 0; attempt < attempts; attempt += 1)); do
        if [[ "$(capture_provider)" == *"$expected"* ]]; then
            return 0
        fi
        sleep 0.2
    done
    return 1
}

wait_for_text_to_disappear() {
    local expected="$1"
    local attempts=$((timeout_seconds * 5))
    local attempt

    for ((attempt = 0; attempt < attempts; attempt += 1)); do
        if [[ "$(capture_provider)" != *"$expected"* ]]; then
            return 0
        fi
        sleep 0.2
    done
    return 1
}

wait_for_event() {
    local event="$1"
    local accepted="$2"
    local attempts=$((timeout_seconds * 5))
    local attempt

    for ((attempt = 0; attempt < attempts; attempt += 1)); do
        if [[ -f "$event_log" ]] &&
            jq -e --arg event "$event" --argjson accepted "$accepted" \
                'select(.event == $event and .accepted == $accepted)' "$event_log" >/dev/null 2>&1; then
            return 0
        fi
        sleep 0.2
    done
    return 1
}

wait_for_clear_rebind() {
    local attempts=$((timeout_seconds * 5))
    local attempt

    for ((attempt = 0; attempt < attempts; attempt += 1)); do
        if [[ -f "$event_log" ]] &&
            jq -e 'select(
                .event == "SessionStart"
                and .accepted == true
                and .source == "clear"
                and .session_changed == true
            )' "$event_log" >/dev/null 2>&1; then
            return 0
        fi
        sleep 0.2
    done
    return 1
}

accepted_event_count() {
    if [[ ! -s "$event_log" ]]; then
        printf '0\n'
        return
    fi
    jq -s '[.[] | select(.accepted == true)] | length' "$event_log"
}

send_literal() {
    private_tmux "$presentation_socket" send-keys -t "$PRESENTATION_SESSION:0.0" -l "$1"
}

send_key() {
    private_tmux "$presentation_socket" send-keys -t "$PRESENTATION_SESSION:0.0" "$1"
}

write_tmux_config() {
    cat >"$tmux_config" <<'EOF'
set -g default-terminal "tmux-256color"
set -g status off
set -g mouse on
set-environment -g COLORTERM truecolor
EOF
    chmod 600 "$tmux_config"
}

write_hook_handler() {
    cat >"$hook_handler" <<'EOF'
#!/usr/bin/env bash
set -u

payload_file="$(mktemp "${WSNAV_SPIKE_ROOT:?}/hook-payload.XXXXXX")" || exit 0
cleanup_payload() {
    rm -f -- "$payload_file"
}
trap cleanup_payload EXIT

# This drain precedes every authority, state, size, and parse decision.
cat >"$payload_file" || true

event="$(jq -r '.hook_event_name // "unknown"' "$payload_file" 2>/dev/null || printf 'malformed')"
payload_cwd="$(jq -r '.cwd // ""' "$payload_file" 2>/dev/null || true)"
session_id="$(jq -r '.session_id // ""' "$payload_file" 2>/dev/null || true)"
turn_id="$(jq -r '.turn_id // ""' "$payload_file" 2>/dev/null || true)"
source="$(jq -r '.source // .reason // ""' "$payload_file" 2>/dev/null || true)"
source_kind="other"
case "$source" in
    startup|resume|clear|compact)
        source_kind="$source"
        ;;
    "")
        source_kind="absent"
        ;;
esac

authority_ok=false
generation_ok=false
cwd_ok=false
ancestry_ok=false
allowed_event=false
accepted=false
session_changed=false
reason="missing-authority"
provider_depth=-1

expected_generation="$(<"${WSNAV_EXPECTED_GENERATION_FILE:?}")"
expected_cwd="$(<"${WSNAV_EXPECTED_CWD_FILE:?}")"
expected_provider_pid="$(<"${WSNAV_PROVIDER_PID_FILE:?}")"

if [[ "${WSNAV_SPIKE_AUTHORITY:-}" == "observer-authority" ]]; then
    authority_ok=true
fi
if [[ "${WSNAV_SPIKE_GENERATION:-}" == "$expected_generation" ]]; then
    generation_ok=true
fi
if [[ "$payload_cwd" == "$expected_cwd" ]]; then
    cwd_ok=true
fi
case "$event" in
    SessionStart|UserPromptSubmit|Stop|SessionEnd)
        allowed_event=true
        ;;
esac

cursor="$PPID"
for ((depth = 0; depth < 8; depth += 1)); do
    if [[ "$cursor" == "$expected_provider_pid" ]]; then
        provider_depth="$depth"
        break
    fi
    [[ -r "/proc/$cursor/stat" ]] || break
    cursor="$(awk '{print $4}' "/proc/$cursor/stat" 2>/dev/null || printf '0')"
    [[ "$cursor" =~ ^[1-9][0-9]*$ ]] || break
done
if ((provider_depth == 0 || provider_depth == 1)); then
    ancestry_ok=true
fi

if [[ "$authority_ok" != true ]]; then
    reason="missing-authority"
elif [[ "$generation_ok" != true ]]; then
    reason="stale-generation"
elif [[ "$cwd_ok" != true ]]; then
    reason="wrong-cwd"
elif [[ "$ancestry_ok" != true ]]; then
    reason="forged-process"
elif [[ "$allowed_event" != true || -z "$session_id" ]]; then
    reason="invalid-event"
else
    event_key="$(printf '%s\0%s\0%s\0%s' "$event" "$session_id" "$turn_id" "$source" | sha256sum | awk '{print $1}')"
    if grep -Fxq "$event_key" "${WSNAV_SEEN_LOG:?}" 2>/dev/null; then
        reason="replay"
    else
        printf '%s\n' "$event_key" >>"${WSNAV_SEEN_LOG:?}"
        if [[ "$event" == "SessionStart" ]]; then
            if [[ -s "${WSNAV_OBSERVED_SESSION_FILE:?}" ]] &&
                [[ "$(<"${WSNAV_OBSERVED_SESSION_FILE:?}")" != "$session_id" ]]; then
                session_changed=true
            fi
            printf '%s\n' "$session_id" >"${WSNAV_OBSERVED_SESSION_FILE:?}"
        fi
        accepted=true
        reason="accepted"
    fi
fi

jq -nc \
    --arg event "$event" \
    --arg source "$source_kind" \
    --arg reason "$reason" \
    --argjson accepted "$accepted" \
    --argjson session_changed "$session_changed" \
    --argjson authority_ok "$authority_ok" \
    --argjson generation_ok "$generation_ok" \
    --argjson cwd_ok "$cwd_ok" \
    --argjson ancestry_ok "$ancestry_ok" \
    --argjson allowed_event "$allowed_event" \
    --argjson provider_depth "$provider_depth" \
    '{
        event: $event,
        source: $source,
        accepted: $accepted,
        session_changed: $session_changed,
        reason: $reason,
        authority_ok: $authority_ok,
        generation_ok: $generation_ok,
        cwd_ok: $cwd_ok,
        ancestry_ok: $ancestry_ok,
        allowed_event: $allowed_event,
        provider_depth: $provider_depth
    }' >>"${WSNAV_EVENT_LOG:?}" 2>/dev/null || true

exit 0
EOF
    chmod 700 "$hook_handler"
}

write_base_config() {
    cat >"$codex_home/config.toml" <<EOF
model_reasoning_effort = "low"

[features]
hooks = false

[projects."$workspace"]
trust_level = "trusted"
EOF
    chmod 600 "$codex_home/config.toml"
}

render_profile() {
    local destination="$1"

    cat >"$destination" <<EOF
# Managed by the isolated Workstream Navigator observer-profile spike.
[features]
hooks = true

[[hooks.SessionStart]]
matcher = "startup|resume|clear|compact"
[[hooks.SessionStart.hooks]]
type = "command"
command = "$hook_handler"
timeout = 3

[[hooks.UserPromptSubmit]]
[[hooks.UserPromptSubmit.hooks]]
type = "command"
command = "$hook_handler"
timeout = 3

[[hooks.Stop]]
[[hooks.Stop.hooks]]
type = "command"
command = "$hook_handler"
timeout = 3

[[hooks.SessionEnd]]
matcher = "other"
[[hooks.SessionEnd.hooks]]
type = "command"
command = "$hook_handler"
timeout = 3
EOF
    chmod 600 "$destination"
}

install_profile_for_test() {
    local source="$1"
    local destination="$2"
    local ownership_record="$3"
    local content_hash

    content_hash="$(sha256sum "$source" | awk '{print $1}')"
    if [[ -e "$destination" ]]; then
        [[ -f "$ownership_record" ]] || return 10
        [[ "$(<"$ownership_record")" == "$content_hash" ]] || return 11
        [[ "$(sha256sum "$destination" | awk '{print $1}')" == "$content_hash" ]] || return 12
    fi
    install -m 600 "$source" "$destination"
    printf '%s\n' "$content_hash" >"$ownership_record"
    chmod 600 "$ownership_record"
}

remove_profile_for_test() {
    local destination="$1"
    local ownership_record="$2"
    local expected_hash

    [[ -f "$destination" && -f "$ownership_record" ]] || return 20
    expected_hash="$(<"$ownership_record")"
    [[ "$(sha256sum "$destination" | awk '{print $1}')" == "$expected_hash" ]] || return 21
    rm -- "$destination" "$ownership_record"
}

start_runtime() {
    local with_profile="$1"
    local runtime_command
    local attach_command

    rm -f -- "$runtime_socket" "$presentation_socket"
    : >"$provider_pid_file"
    if [[ "$with_profile" == true ]]; then
        printf -v runtime_command \
            'printf "%%s\n" "$$" > %q; exec env CODEX_HOME=%q COLORTERM=truecolor WSNAV_SPIKE_ROOT=%q WSNAV_SPIKE_AUTHORITY=observer-authority WSNAV_SPIKE_GENERATION=gen-live WSNAV_EXPECTED_GENERATION_FILE=%q WSNAV_EXPECTED_CWD_FILE=%q WSNAV_PROVIDER_PID_FILE=%q WSNAV_EVENT_LOG=%q WSNAV_SEEN_LOG=%q WSNAV_OBSERVED_SESSION_FILE=%q codex --profile %q -s read-only -a never -C %q' \
            "$provider_pid_file" "$codex_home" "$spike_root" "$expected_generation_file" "$expected_cwd_file" "$provider_pid_file" "$event_log" "$seen_log" "$observed_session_file" "$PROFILE_NAME" "$workspace"
    else
        printf -v runtime_command \
            'printf "%%s\n" "$$" > %q; exec env CODEX_HOME=%q COLORTERM=truecolor codex -s read-only -a never -C %q' \
            "$provider_pid_file" "$codex_home" "$workspace"
    fi

    private_tmux "$runtime_socket" -f "$tmux_config" new-session -d -s "$RUNTIME_SESSION" -x 120 -y 42 "$runtime_command"
    runtime_server_started=true
    printf -v attach_command 'exec env -u TMUX tmux -S %q attach-session -t %q' "$runtime_socket" "$RUNTIME_SESSION"
    private_tmux "$presentation_socket" -f "$tmux_config" new-session -d -s "$PRESENTATION_SESSION" -x 140 -y 44 "$attach_command"
    presentation_server_started=true
}

wait_for_runtime_exit() {
    local attempts=$((timeout_seconds * 5))
    local attempt

    for ((attempt = 0; attempt < attempts; attempt += 1)); do
        if ! private_tmux "$runtime_socket" has-session -t "$RUNTIME_SESSION" >/dev/null 2>&1; then
            return 0
        fi
        sleep 0.2
    done
    return 1
}

exit_native_tui() {
    send_literal "/exit"
    sleep 0.2
    send_key C-m
    wait_for_runtime_exit
}

write_result() {
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
    "id": "codex",
    "version": "$provider_version",
    "contract_fingerprint": "$contract_fingerprint"
  },
  "status": "$status",
  "reason": "$reason",
  "assertions": {
    "profile_layers_over_base": $profile_layers_over_base,
    "base_config_preserved": $base_config_preserved,
    "native_hook_trust_confirmed": $native_hook_trust_confirmed,
    "promptless_trust_review_confirmed": $promptless_trust_review_confirmed,
    "promptless_review_left_native_session": $promptless_review_left_native_session,
    "session_start_observed": $session_start_observed,
    "user_prompt_submit_observed": $user_prompt_submit_observed,
    "stop_observed": $stop_observed,
    "session_end_observed": $session_end_observed,
    "lifecycle_order_confirmed": $lifecycle_order_confirmed,
    "clear_rebind_observed": $clear_rebind_observed,
    "ordinary_launch_unobserved": $ordinary_launch_unobserved,
    "trusted_profile_reused": $trusted_profile_reused,
    "large_unmanaged_payload_drained": $large_unmanaged_payload_drained,
    "missing_authority_rejected": $missing_authority_rejected,
    "stale_generation_rejected": $stale_generation_rejected,
    "forged_process_rejected": $forged_process_rejected,
    "profile_collision_refused": $profile_collision_refused,
    "modified_profile_removal_refused": $modified_profile_removal_refused,
    "exact_profile_removal_succeeds": $exact_profile_removal_succeeds,
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
    local cleanup_status=complete
    local ordinary_tmux_unchanged=false
    local ordinary_tmux_after

    cleanup
    [[ "$cleanup_complete" == true ]] || cleanup_status=incomplete
    ordinary_tmux_after="$(ordinary_tmux_fingerprint)"
    if [[ "$ordinary_tmux_after" == "$ordinary_tmux_before" ]]; then
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
    write_result "$status" "$reason" "$ordinary_tmux_unchanged" "$cleanup_status"
    exit "$exit_code"
}

trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

start_seconds="$(date +%s)"
ordinary_tmux_before="$(ordinary_tmux_fingerprint)"
provider_version="$(codex --version | awk 'NR == 1 { print $2 }' | tr -cd '[:alnum:].+-')"
[[ -n "$provider_version" ]] || provider_version=unknown

spike_root="$(mktemp -d /tmp/wsnav-codex-observer-spike.XXXXXX)"
chmod 700 "$spike_root"
codex_home="$spike_root/codex-home"
workspace="$spike_root/workspace"
runtime_socket="$spike_root/runtime.sock"
presentation_socket="$spike_root/presentation.sock"
tmux_config="$spike_root/tmux.conf"
hook_handler="$spike_root/observer-hook"
event_log="$spike_root/events.jsonl"
seen_log="$spike_root/seen"
provider_pid_file="$spike_root/provider.pid"
expected_cwd_file="$spike_root/expected-cwd"
expected_generation_file="$spike_root/expected-generation"
observed_session_file="$spike_root/observed-session"
mkdir -m 700 "$codex_home" "$workspace"
install -m 600 "$HOME/.codex/auth.json" "$codex_home/auth.json"
git -C "$workspace" init -q
printf '%s\n' "$workspace" >"$expected_cwd_file"
printf '%s\n' "gen-live" >"$expected_generation_file"
: >"$event_log"
: >"$seen_log"
: >"$observed_session_file"
chmod 600 "$event_log" "$seen_log" "$expected_cwd_file" "$expected_generation_file" "$observed_session_file"
write_tmux_config
write_hook_handler
write_base_config

profile_source="$spike_root/profile.generated"
profile_path="$codex_home/$PROFILE_NAME.config.toml"
ownership_record="$spike_root/profile.owner"
render_profile "$profile_source"
install_profile_for_test "$profile_source" "$profile_path" "$ownership_record" ||
    finish falsified owned-profile-install-failed 1
base_config_hash="$(sha256sum "$codex_home/config.toml" | awk '{print $1}')"

collision_home="$spike_root/collision-home"
mkdir -m 700 "$collision_home"
printf '%s\n' "foreign = true" >"$collision_home/$PROFILE_NAME.config.toml"
if ! install_profile_for_test "$profile_source" "$collision_home/$PROFILE_NAME.config.toml" "$collision_home/owner"; then
    profile_collision_refused=true
else
    finish falsified foreign-profile-collision-not-refused 1
fi

modified_home="$spike_root/modified-home"
mkdir -m 700 "$modified_home"
install_profile_for_test "$profile_source" "$modified_home/$PROFILE_NAME.config.toml" "$modified_home/owner" ||
    finish falsified modified-profile-setup-failed 1
printf '\n# user modification\n' >>"$modified_home/$PROFILE_NAME.config.toml"
if ! remove_profile_for_test "$modified_home/$PROFILE_NAME.config.toml" "$modified_home/owner"; then
    modified_profile_removal_refused=true
else
    finish falsified modified-profile-removal-not-refused 1
fi

exact_home="$spike_root/exact-home"
mkdir -m 700 "$exact_home"
install_profile_for_test "$profile_source" "$exact_home/$PROFILE_NAME.config.toml" "$exact_home/owner" ||
    finish falsified exact-profile-setup-failed 1
remove_profile_for_test "$exact_home/$PROFILE_NAME.config.toml" "$exact_home/owner" ||
    finish falsified exact-profile-removal-failed 1
exact_profile_removal_succeeds=true

# The first profile-selected process is the promptless native trust review.
start_runtime true || finish blocked profile-trust-runtime-start-failed 2
wait_for_text 'Trust all and continue' ||
    finish falsified native-hook-trust-review-not-visible 1
send_key Down
send_key C-m
wait_for_text_to_disappear 'Trust all and continue' ||
    finish falsified native-hook-trust-not-accepted 1
native_hook_trust_confirmed=true
exit_native_tui || finish falsified trust-review-tui-did-not-exit 1
promptless_trust_review_confirmed=true
cleanup_runtime

if find "$codex_home/sessions" -type f -print -quit 2>/dev/null | grep -q .; then
    promptless_review_left_native_session=true
fi

# Trust must persist and the remaining turn lifecycle must be passive.
start_runtime true || finish falsified trusted-profile-reuse-start-failed 1
sleep 2
if [[ "$(capture_provider)" == *"Trust all and continue"* ]]; then
    finish falsified trusted-profile-prompted-again 1
fi
trusted_profile_reused=true
send_literal "$RESULT_PROMPT"
sleep 0.2
send_key C-m
wait_for_text "$RESULT_MARKER" ||
    finish falsified harmless-managed-turn-did-not-complete 1
wait_for_event SessionStart true ||
    finish falsified reused-profile-session-start-not-observed 1
session_start_observed=true
profile_layers_over_base=true
wait_for_event UserPromptSubmit true ||
    finish falsified user-prompt-submit-not-observed 1
user_prompt_submit_observed=true
wait_for_event Stop true ||
    finish falsified stop-not-observed 1
stop_observed=true
if jq -se '
    [.[] | select(.accepted == true) | .event] as $events
    | ($events | index("SessionStart")) as $start
    | ($events | index("UserPromptSubmit")) as $prompt
    | ($events | index("Stop")) as $stop
    | $start != null and $prompt != null and $stop != null
      and $start < $prompt and $prompt < $stop
' "$event_log" >/dev/null; then
    lifecycle_order_confirmed=true
else
    finish falsified managed-lifecycle-order-invalid 1
fi

# Codex owns the /clear action. This probe establishes whether it creates a
# distinct native session in the existing TUI and which SessionStart source
# describes it. It records only a boolean relationship, never either ID.
send_literal "/clear"
sleep 0.2
send_key C-m
if ! wait_for_clear_rebind; then
    # Current Codex creates a conversation lazily. If /clear only resets the
    # landing screen, make one harmless native turn to cause its destination
    # thread to exist without sending management input through WSNav.
    send_literal "$RESULT_PROMPT"
    sleep 0.2
    send_key C-m
    wait_for_text "$RESULT_MARKER" ||
        finish falsified clear-destination-turn-did-not-complete 1
    wait_for_clear_rebind ||
        finish falsified native-clear-did-not-produce-a-distinct-clear-session 1
fi
clear_rebind_observed=true

# Full authority values from a non-provider process must still be rejected.
printf '%s\n' '{"hook_event_name":"Stop","session_id":"forged","turn_id":"forged","cwd":"'"$workspace"'"}' |
    env \
        WSNAV_SPIKE_ROOT="$spike_root" \
        WSNAV_SPIKE_AUTHORITY=observer-authority \
        WSNAV_SPIKE_GENERATION=gen-live \
        WSNAV_EXPECTED_GENERATION_FILE="$expected_generation_file" \
        WSNAV_EXPECTED_CWD_FILE="$expected_cwd_file" \
        WSNAV_PROVIDER_PID_FILE="$provider_pid_file" \
        WSNAV_EVENT_LOG="$event_log" \
        WSNAV_SEEN_LOG="$seen_log" \
        "$hook_handler"
wait_for_event Stop false || finish falsified forged-hook-invocation-not-recorded 1
if jq -e 'select(.reason == "forged-process" and .accepted == false)' "$event_log" >/dev/null; then
    forged_process_rejected=true
else
    finish falsified forged-process-not-rejected 1
fi

printf '%s\n' '{"hook_event_name":"Stop","session_id":"stale","turn_id":"stale","cwd":"'"$workspace"'"}' |
    env \
        WSNAV_SPIKE_ROOT="$spike_root" \
        WSNAV_SPIKE_AUTHORITY=observer-authority \
        WSNAV_SPIKE_GENERATION=gen-stale \
        WSNAV_EXPECTED_GENERATION_FILE="$expected_generation_file" \
        WSNAV_EXPECTED_CWD_FILE="$expected_cwd_file" \
        WSNAV_PROVIDER_PID_FILE="$provider_pid_file" \
        WSNAV_EVENT_LOG="$event_log" \
        WSNAV_SEEN_LOG="$seen_log" \
        "$hook_handler"
if jq -e 'select(.reason == "stale-generation" and .accepted == false)' "$event_log" >/dev/null; then
    stale_generation_rejected=true
else
    finish falsified stale-generation-not-rejected 1
fi

set +e
awk 'BEGIN {
    printf "{\"hook_event_name\":\"Stop\",\"session_id\":\"unmanaged\",\"cwd\":\"/unmanaged\",\"pad\":\""
    for (i = 0; i < 300000; i += 1) {
        printf "x"
    }
    print "\"}"
}' |
    env \
        -u WSNAV_SPIKE_AUTHORITY \
        WSNAV_SPIKE_ROOT="$spike_root" \
        WSNAV_SPIKE_GENERATION=gen-live \
        WSNAV_EXPECTED_GENERATION_FILE="$expected_generation_file" \
        WSNAV_EXPECTED_CWD_FILE="$expected_cwd_file" \
        WSNAV_PROVIDER_PID_FILE="$provider_pid_file" \
        WSNAV_EVENT_LOG="$event_log" \
        WSNAV_SEEN_LOG="$seen_log" \
        "$hook_handler"
pipeline_status=("${PIPESTATUS[@]}")
set -e
if [[ "${pipeline_status[0]}" == 0 && "${pipeline_status[1]}" == 0 ]]; then
    large_unmanaged_payload_drained=true
else
    finish falsified large-unmanaged-payload-broke-pipe 1
fi
if jq -e 'select(.reason == "missing-authority" and .accepted == false)' "$event_log" >/dev/null; then
    missing_authority_rejected=true
else
    finish falsified missing-authority-not-rejected 1
fi

exit_native_tui || finish falsified managed-turn-tui-did-not-exit 1
wait_for_event SessionEnd true ||
    finish falsified trusted-session-end-not-observed 1
session_end_observed=true
cleanup_runtime

# The installed profile must be inert unless explicitly selected.
accepted_before_ordinary="$(accepted_event_count)"
start_runtime false || finish falsified ordinary-launch-start-failed 1
sleep 3
if [[ "$(capture_provider)" == *"Trust all and continue"* ]]; then
    finish falsified ordinary-launch-saw-observer-trust 1
fi
exit_native_tui || finish falsified ordinary-tui-did-not-exit 1
cleanup_runtime
accepted_after_ordinary="$(accepted_event_count)"
if [[ "$accepted_before_ordinary" == "$accepted_after_ordinary" ]]; then
    ordinary_launch_unobserved=true
else
    finish falsified ordinary-launch-produced-observer-event 1
fi

if [[ "$(sha256sum "$codex_home/config.toml" | awk '{print $1}')" != "$base_config_hash" ]]; then
    finish falsified base-config-was-modified 1
fi
base_config_preserved=true

finish pass scoped-profile-and-hook-authority-proven 0
