#!/usr/bin/env bash
# Disposable D8.2 lifecycle-correctness acceptance.  It exercises the
# conclusive-loss, attachment, recovery, and archive boundaries with a fake
# Codex provider.  Every state root, Codex home, checkout, tmux socket, and
# helper process is owned by this invocation and removed by the trap.
set -euo pipefail

workspace_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
task_root="$(mktemp -d)"
ordinary_tmux_before="$(env -u TMUX tmux list-sessions -F '#{session_name}:#{session_created}:#{session_windows}' -O name 2>/dev/null || true)"
wsnav_bin=""
state_root=""
workstream_id=""
helper_dir=""
proc_evidence=""

process_state() {
    local pid="$1"
    python3 - "$pid" <<'PY'
from pathlib import Path
import sys

try:
    stat = Path(f"/proc/{int(sys.argv[1])}/stat").read_text(encoding="utf-8")
except (FileNotFoundError, OSError):
    raise SystemExit(1)
fields = stat.rsplit(")", 1)[-1].strip().split()
if not fields:
    raise SystemExit(1)
print(fields[0])
PY
}

process_birth() {
    local pid="$1"
    python3 - "$pid" <<'PY'
from pathlib import Path
import sys

try:
    stat = Path(f"/proc/{int(sys.argv[1])}/stat").read_text(encoding="utf-8")
    close_paren = stat.rfind(")")
    print(stat[close_paren + 2 :].split()[19])
except (FileNotFoundError, IndexError, OSError):
    raise SystemExit(1)
PY
}

assert_process_gone() {
    local label="$1" pid="$2" expected_birth="$3"
    if [[ -z "$pid" ]]; then
        return
    fi
    local state
    # A missing or changed birth token is safe evidence that the recorded
    # helper instance is already gone.  Never signal a reused numeric PID.
    if [[ "$(process_birth "$pid" 2>/dev/null || true)" != "$expected_birth" ]]; then
        return
    fi
    state="$(process_state "$pid" 2>/dev/null || true)"
    if [[ -n "$state" && "$state" != Z ]]; then
        echo "$label process survived lifecycle cleanup (state=$state)" >&2
        emit_relational_evidence
        return 1
    fi
}

capture_process_identity() {
    local label="$1" pid="$2"
    python3 - "$proc_evidence" "$label" "$pid" <<'PY'
import json
from pathlib import Path
import sys

path, label, pid_text = sys.argv[1:]
pid = int(pid_text)
try:
    stat = Path(f"/proc/{pid}/stat").read_text(encoding="utf-8")
    close_paren = stat.rfind(")")
    fields = stat[close_paren + 2 :].split()
    value = {
        "pid": pid,
        "birth": fields[19],
        "ppid": fields[1],
        "pgrp": fields[2],
        "session": fields[3],
        "state": fields[0],
    }
except (FileNotFoundError, IndexError, OSError):
    value = {"pid": pid, "missing": True}
data = json.loads(Path(path).read_text(encoding="utf-8")) if Path(path).exists() else {}
data[label] = value
Path(path).write_text(json.dumps(data, sort_keys=True), encoding="utf-8")
PY
}

emit_relational_evidence() {
    python3 - "$proc_evidence" <<'PY' >&2
import json
from pathlib import Path
import sys

path = Path(sys.argv[1])
if not path.exists():
    print("process ownership evidence unavailable")
    raise SystemExit(0)
data = json.loads(path.read_text(encoding="utf-8"))
entries = list(data.items())
provider = next((value for key, value in reversed(entries) if key.endswith("provider")), None)
helper = next((value for key, value in reversed(entries) if key.endswith("helper")), None)
pane = next((value for key, value in reversed(entries) if key.endswith("pane")), None)
if not provider or not helper:
    print("process ownership evidence incomplete")
    raise SystemExit(0)
same_group = provider.get("pgrp") == helper.get("pgrp")
same_session = provider.get("session") == helper.get("session")
pane_group = pane and pane.get("pgrp") == provider.get("pgrp")
pane_session = pane and pane.get("session") == provider.get("session")
helper_live = not helper.get("missing") and helper.get("state") != "Z"
print(
    "process ownership evidence: "
    f"provider/helper same process group={same_group}; "
    f"provider/helper same session={same_session}; "
    f"pane/provider same process group={pane_group}; "
    f"pane/provider same session={pane_session}; "
    f"descendant currently live={helper_live}"
)
PY
}

helper_records() {
    [[ -d "$helper_dir" ]] || return 0
    find "$helper_dir" -type f -maxdepth 1 -print0 2>/dev/null |
        while IFS= read -r -d '' path; do
            tr -d '[:space:]' <"$path"
            printf '\n'
        done
}

latest_helper_record() {
    local path
    path="$(find "$helper_dir" -maxdepth 1 -type f -printf '%p\n' 2>/dev/null | sort | tail -n 1)"
    [[ -n "$path" ]] || return 1
    tr -d '[:space:]' <"$path"
}

kill_helpers() {
    local pid expected_birth current_birth
    while IFS='|' read -r pid expected_birth; do
        [[ "$pid" =~ ^[0-9]+$ && "$expected_birth" =~ ^[0-9]+$ ]] || continue
        current_birth="$(process_birth "$pid" 2>/dev/null || true)"
        [[ "$current_birth" == "$expected_birth" ]] || continue
        kill -KILL "$pid" >/dev/null 2>&1 || true
    done < <(helper_records)
}

cleanup() {
    # Preserve the original assertion outcome: this trap is only a bounded
    # best-effort cleanup path, never a substitute for the leak checks below.
    if [[ -n "$wsnav_bin" && -n "$state_root" && -n "$workstream_id" ]]; then
        "$wsnav_bin" --state-root "$state_root" park "$workstream_id" >/dev/null 2>&1 || true
    fi
    if [[ -n "$state_root" && -d "$state_root/run" ]]; then
        while IFS= read -r socket; do
            env -u TMUX tmux -S "$socket" kill-server >/dev/null 2>&1 || true
        done < <(find "$state_root/run" -type s -name tmux.sock -print 2>/dev/null)
    fi
    kill_helpers
    rm -rf -- "$task_root"
}
trap cleanup EXIT

cargo build --quiet
wsnav_bin="$workspace_root/target/debug/wsnav"
fake_bin="$task_root/bin"
state_root="$task_root/state"
codex_home="$task_root/codex-home"
repository="$task_root/repository"
helper_dir="$task_root/helpers"
proc_evidence="$task_root/process-identity.json"
mkdir -p "$fake_bin" "$codex_home" "$repository" "$helper_dir"

cat >"$fake_bin/codex" <<'SH'
#!/usr/bin/env bash
set -euo pipefail

if [[ "${1:-}" == "--version" ]]; then
    printf 'codex d8.2 fixture\n'
    exit 0
fi

if [[ "${1:-}" == "app-server" ]]; then
    while IFS= read -r line; do
        case "$line" in
            *'"id":1'*) printf '%s\n' '{"id":1,"result":{}}' ;;
            *'"id":2'*'thread/read'*) printf '%s\n' '{"id":2,"result":{"thread":{"id":"d8.2-session","name":"lifecycle fixture"}}}' ;;
            *'"id":2'*'thread/name/set'*) printf '%s\n' '{"id":2,"result":{}}' ;;
        esac
    done
    exit 0
fi

# A provider can outlive its private tmux server.  Ignore the terminal hangup
# so recovery must prove and clean the persisted PID-plus-birth identity.
trap '' HUP
helper_path="${FAKE_HELPER_DIR}/provider-${BASHPID}"
python3 -c 'import signal,time; signal.signal(signal.SIGHUP, signal.SIG_IGN); signal.signal(signal.SIGTERM, signal.SIG_IGN); signal.signal(signal.SIGINT, signal.SIG_IGN); [time.sleep(1) for _ in iter(int, 1)]' &
helper_pid=$!
helper_birth="$(python3 - "$helper_pid" <<'PY'
from pathlib import Path
import sys

stat = Path(f"/proc/{int(sys.argv[1])}/stat").read_text(encoding="utf-8")
close_paren = stat.rfind(")")
print(stat[close_paren + 2 :].split()[19])
PY
)"
printf '%s|%s\n' "$helper_pid" "$helper_birth" >"$helper_path"

source_value=startup
if [[ " $* " == *" resume "* ]]; then
    source_value=resume
fi
printf '{"hook_event_name":"SessionStart","cwd":"%s","session_id":"d8.2-session","source":"%s"}' "$PWD" "$source_value" | wsnav --state-root "$FAKE_STATE_ROOT" _hook
printf '{"hook_event_name":"UserPromptSubmit","cwd":"%s","session_id":"d8.2-session","turn_id":"d8.2-turn"}' "$PWD" | wsnav --state-root "$FAKE_STATE_ROOT" _hook
printf '{"hook_event_name":"Stop","cwd":"%s","session_id":"d8.2-session","turn_id":"d8.2-turn"}' "$PWD" | wsnav --state-root "$FAKE_STATE_ROOT" _hook
printf 'D8.2_FAKE_CODEX_NATIVE_SURFACE\n'
sleep 300
SH
chmod 700 "$fake_bin/codex"

git -C "$repository" init -q -b main
git -C "$repository" config user.name wsnav-test
git -C "$repository" config user.email wsnav@example.test
printf 'base\n' >"$repository/README"
git -C "$repository" add README
git -C "$repository" commit -qm initial

export CODEX_HOME="$codex_home"
export FAKE_STATE_ROOT="$state_root"
export FAKE_HELPER_DIR="$helper_dir"
export PATH="$fake_bin:$workspace_root/target/debug:$PATH"

# Install and explicitly trust the disposable observer profile.  The profile
# is never written to the user's normal Codex home.
"$wsnav_bin" --state-root "$state_root" setup --skip-review
profile_path="$codex_home/wsnav-observer.config.toml"
for hook in session_start user_prompt_submit stop session_end; do
    printf '\n[hooks.state."%s:%s:0:0"]\ntrusted_hash = "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"\n' \
        "$profile_path" "$hook" >>"$profile_path"
done
"$wsnav_bin" --state-root "$state_root" trust-observer
registration="$($wsnav_bin --state-root "$state_root" register --provider codex "$repository")"
workstream_id="${registration##* }"

runtime_row() {
    python3 - "$state_root/host.sqlite" "$workstream_id" <<'PY'
import sqlite3
import sys

row = sqlite3.connect(sys.argv[1]).execute(
    """SELECT runtime_id, provider_pid, process_birth, lifecycle, revision,
                      tmux_session
           FROM runtimes WHERE workstream_id = ?""",
    (sys.argv[2],),
).fetchone()
if row is None:
    raise SystemExit("missing Runtime record")
print("|".join("" if value is None else str(value) for value in row))
PY
}

workstream_revision() {
    python3 - "$state_root/host.sqlite" "$workstream_id" <<'PY'
import sqlite3
import sys

value = sqlite3.connect(sys.argv[1]).execute(
    "SELECT revision FROM workstreams WHERE workstream_id = ?", (sys.argv[2],)
).fetchone()
if value is None:
    raise SystemExit("missing Workstream record")
print(value[0])
PY
}

wait_status_contains() {
    local text="$1"
    for _ in {1..100}; do
        if "$wsnav_bin" --state-root "$state_root" status "$workstream_id" 2>/dev/null | grep -F "$text" >/dev/null; then
            return 0
        fi
        sleep 0.1
    done
    echo "timed out waiting for status marker: $text" >&2
    "$wsnav_bin" --state-root "$state_root" status "$workstream_id" >&2 || true
    return 1
}

"$wsnav_bin" --state-root "$state_root" start "$workstream_id"
wait_status_contains 'lifecycle: Attention'
IFS='|' read -r first_runtime first_pid first_birth first_lifecycle _ first_session <<<"$(runtime_row)"
[[ "$first_lifecycle" != stopped && -n "$first_pid" && -n "$first_birth" ]]
runtime_socket="$state_root/run/runtime-$first_runtime/tmux.sock"
[[ -S "$runtime_socket" ]]
pane_pid="$(env -u TMUX tmux -S "$runtime_socket" display-message -p -t "$first_session:0.0" '#{pane_pid}')"
[[ "$pane_pid" == "$first_pid" ]]
IFS='|' read -r first_helper first_helper_birth <<<"$(find "$helper_dir" -maxdepth 1 -type f -exec cat {} \; -quit)"
[[ "$first_helper" =~ ^[0-9]+$ && "$first_helper_birth" =~ ^[0-9]+$ ]]
process_state "$first_pid" >/dev/null
capture_process_identity first-provider "$first_pid"
capture_process_identity first-pane "$pane_pid"
capture_process_identity first-helper "$first_helper"

# Loss of the exact private tmux server must become visible.  Direct attach is
# deliberately attempted before recovery; the failed preflight itself must
# promote the ordinary Workstream to RecoveryRequired instead of returning a
# repeatable generic attachment error.
env -u TMUX tmux -S "$runtime_socket" kill-server
wait_status_contains 'workstream: RecoveryRequired'
if "$wsnav_bin" --state-root "$state_root" attach "$workstream_id" \
    >"$task_root/attach-missing.out" 2>&1; then
    echo 'attach unexpectedly succeeded after private tmux loss' >&2
    exit 1
fi
wait_status_contains 'workstream: RecoveryRequired'
wait_status_contains 'recovery attention: unseen'

# Recovery must stop the exact old provider process before reserving the next
# Runtime generation.  The helper intentionally ignores TERM/HUP; its survival
# is a process-tree ownership falsification, not something the trap may hide.
"$wsnav_bin" --state-root "$state_root" recover "$workstream_id"
wait_status_contains 'workstream: Open'
IFS='|' read -r second_runtime second_pid second_birth _ _ _ <<<"$(runtime_row)"
[[ "$second_runtime" == "$first_runtime" && "$second_pid" != "$first_pid" ]]
assert_process_gone first-provider "$first_pid" "$first_birth"
assert_process_gone first-helper "$first_helper" "$first_helper_birth"

# Simulate a stopped Runtime whose private artifacts and provider are still
# present.  Archive must park it anyway; lifecycle state alone cannot justify
# skipping owned-process cleanup.
archive_revision="$(workstream_revision)"
python3 - "$state_root/host.sqlite" "$second_runtime" <<'PY'
import sqlite3
import sys

connection = sqlite3.connect(sys.argv[1])
connection.execute(
    "UPDATE runtimes SET lifecycle = 'stopped', revision = revision + 1 WHERE runtime_id = ?",
    (sys.argv[2],),
)
connection.commit()
PY
[[ -S "$state_root/run/runtime-$second_runtime/tmux.sock" ]]
IFS='|' read -r second_helper second_helper_birth <<<"$(latest_helper_record)"
[[ "$second_helper" =~ ^[0-9]+$ && "$second_helper_birth" =~ ^[0-9]+$ ]]
second_socket="$state_root/run/runtime-$second_runtime/tmux.sock"
second_session_name="$(env -u TMUX tmux -S "$second_socket" list-sessions -F '#{session_name}' | head -n 1)"
second_pane_pid="$(env -u TMUX tmux -S "$second_socket" display-message -p -t "$second_session_name:0.0" '#{pane_pid}')"
[[ "$second_pane_pid" == "$second_pid" ]]
capture_process_identity second-provider "$second_pid"
capture_process_identity second-pane "$second_pane_pid"
capture_process_identity second-helper "$second_helper"
"$wsnav_bin" --state-root "$state_root" archive "$workstream_id" "$archive_revision"
assert_process_gone second-provider "$second_pid" "$second_birth"
assert_process_gone second-helper "$second_helper" "$second_helper_birth"
[[ ! -e "$state_root/run/runtime-$second_runtime" ]]
python3 - "$state_root/host.sqlite" "$workstream_id" <<'PY'
import sqlite3
import sys

connection = sqlite3.connect(sys.argv[1])
workstream = connection.execute(
    "SELECT lifecycle, archived_at_millis FROM workstreams WHERE workstream_id = ?",
    (sys.argv[2],),
).fetchone()
runtime = connection.execute(
    "SELECT lifecycle FROM runtimes WHERE workstream_id = ?", (sys.argv[2],)
).fetchone()
if workstream is None or workstream[0] != "parked" or workstream[1] is None:
    raise SystemExit(f"archive did not persist visibility transition: {workstream}")
if runtime != ("stopped",):
    raise SystemExit(f"archive did not leave Runtime stopped: {runtime}")
PY

workstream_id=""
ordinary_tmux_after="$(env -u TMUX tmux list-sessions -F '#{session_name}:#{session_created}:#{session_windows}' -O name 2>/dev/null || true)"
[[ "$ordinary_tmux_before" == "$ordinary_tmux_after" ]]
while IFS='|' read -r helper_pid helper_birth; do
    [[ "$helper_pid" =~ ^[0-9]+$ && "$helper_birth" =~ ^[0-9]+$ ]] || continue
    assert_process_gone final-helper "$helper_pid" "$helper_birth"
done < <(helper_records)
trap - EXIT
rm -rf -- "$task_root"
printf 'D8.2 disposable lifecycle-correctness acceptance passed\n'
