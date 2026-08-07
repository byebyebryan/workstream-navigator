#!/usr/bin/env bash
# Disposable D8.2 crash-boundary acceptance. It kills only the owning wsnav
# Start action after the durable OpenCode Start boundary and proves that the
# crash-surviving guardian removes its temporary provider process group.
set -euo pipefail

workspace_root="$(cd "$(dirname "$0")/.." && pwd)"
task_root="$(mktemp -d)"
ordinary_tmux_before="$(env -u TMUX tmux list-sessions -F '#{session_name}:#{session_created}:#{session_windows}' -O name 2>/dev/null || true)"
wsnav_bin=""
state_root=""
workstream_id=""
action_pid=""
action_birth=""
guardian_pid=""
guardian_birth=""
helper_pid=""
helper_birth=""
descendant_pid=""
descendant_birth=""
sentinel_pid=""
sentinel_birth=""
helper_group=""
helper_session=""
helper_record=""

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

process_state() {
    local pid="$1"
    python3 - "$pid" <<'PY'
from pathlib import Path
import sys

try:
    stat = Path(f"/proc/{int(sys.argv[1])}/stat").read_text(encoding="utf-8")
    print(stat.rsplit(")", 1)[-1].strip().split()[0])
except (FileNotFoundError, IndexError, OSError):
    raise SystemExit(1)
PY
}

capture_process() {
    local label="$1" pid="$2"
    python3 - "$pid" >"$task_root/process-$label.json" <<'PY'
import json
from pathlib import Path
import sys

pid = int(sys.argv[1])
stat = Path(f"/proc/{pid}/stat").read_text(encoding="utf-8")
close_paren = stat.rfind(")")
fields = stat[close_paren + 2 :].split()
print(json.dumps({
    "pid": pid,
    "birth": fields[19],
    "ppid": int(fields[1]),
    "pgrp": int(fields[2]),
    "session": int(fields[3]),
    "state": fields[0],
}, sort_keys=True))
PY
}

assert_process_live() {
    local label="$1" pid="$2" expected_birth="$3"
    local actual_birth state
    actual_birth="$(process_birth "$pid" 2>/dev/null || true)"
    state="$(process_state "$pid" 2>/dev/null || true)"
    if [[ -z "$pid" || "$actual_birth" != "$expected_birth" || -z "$state" || "$state" == Z ]]; then
        echo "$label process is not the recorded live instance (pid=$pid)" >&2
        return 1
    fi
}

assert_process_gone() {
    local label="$1" pid="$2" expected_birth="$3"
    [[ -n "$pid" && -n "$expected_birth" ]] || return 0
    local state current_birth=""
    for _ in {1..200}; do
        current_birth="$(process_birth "$pid" 2>/dev/null || true)"
        if [[ "$current_birth" != "$expected_birth" ]]; then
            return 0
        fi
        state="$(process_state "$pid" 2>/dev/null || true)"
        if [[ -z "$state" || "$state" == Z ]]; then
            return 0
        fi
        sleep 0.05
    done
    echo "$label process survived guardian cleanup (pid=$pid state=$state)" >&2
    return 1
}

safe_kill_exact() {
    local pid="$1" expected_birth="$2"
    [[ "$pid" =~ ^[0-9]+$ && "$expected_birth" =~ ^[0-9]+$ ]] || return 0
    [[ "$(process_birth "$pid" 2>/dev/null || true)" == "$expected_birth" ]] || return 0
    kill -KILL "$pid" >/dev/null 2>&1 || true
}

assert_group_empty() {
    local expected_group="$1" expected_session="$2"
    python3 - "$expected_group" "$expected_session" <<'PY'
from pathlib import Path
import sys

group, session = int(sys.argv[1]), int(sys.argv[2])
live = []
for entry in Path("/proc").iterdir():
    if not entry.name.isdigit():
        continue
    try:
        stat = (entry / "stat").read_text(encoding="utf-8")
        close_paren = stat.rfind(")")
        fields = stat[close_paren + 2 :].split()
        if int(fields[2]) == group and int(fields[3]) == session and fields[0] != "Z":
            live.append((int(entry.name), fields[0]))
    except (FileNotFoundError, IndexError, OSError, ValueError):
        continue
if live:
    raise SystemExit(f"owned helper process group still has live members: {live}")
PY
}

assert_process_relations() {
    python3 - "$task_root/process-helper.json" "$task_root/process-descendant.json" "$task_root/process-guardian.json" "$task_root/process-action.json" "$task_root/process-sentinel.json" <<'PY'
import json
from pathlib import Path
import sys

helper, descendant, guardian, action, sentinel = [
    json.loads(Path(path).read_text(encoding="utf-8")) for path in sys.argv[1:]
]
if helper["pgrp"] != helper["pid"]:
    raise SystemExit("temporary serve leader did not lead its own process group")
if descendant["pgrp"] != helper["pgrp"] or descendant["session"] != helper["session"]:
    raise SystemExit("TERM/HUP-ignoring descendant was not in the owned group/session")
if helper["ppid"] != guardian["pid"]:
    raise SystemExit("temporary serve leader was not owned directly by the guardian")
if guardian["pid"] == action["pid"]:
    raise SystemExit("guardian identity unexpectedly aliases the killed Start action")
if guardian["pgrp"] != guardian["pid"] or guardian["pgrp"] == action["pgrp"]:
    raise SystemExit("guardian was not isolated from the Start action process group")
if sentinel["pgrp"] in {helper["pgrp"], guardian["pgrp"]}:
    raise SystemExit("unrelated sentinel shared the provider/guardian process group")
PY
}

fake_state() {
    python3 - "$FAKE_OPENCODE_DB" <<'PY'
import json
import sys
from pathlib import Path

try:
    print(json.dumps(json.loads(Path(sys.argv[1]).read_text(encoding="utf-8")), sort_keys=True))
except (FileNotFoundError, json.JSONDecodeError):
    print("{}")
PY
}

journal_state() {
    python3 - "$state_root/host.sqlite" "$workstream_id" <<'PY'
import json
import sqlite3
import sys

database, workstream = sys.argv[1:]
connection = sqlite3.connect(database)
runtime = connection.execute(
    "SELECT runtime_id, tmux_generation, lifecycle, provider_pid, process_birth FROM runtimes WHERE workstream_id = ?",
    (workstream,),
).fetchone()
operation = connection.execute(
    "SELECT operation_id, phase, outcome_json, effect_watermark, revision FROM compound_operations WHERE kind = 'start' ORDER BY rowid DESC LIMIT 1"
).fetchone()
workstream_row = connection.execute(
    "SELECT lifecycle FROM workstreams WHERE workstream_id = ?", (workstream,)
).fetchone()
print(json.dumps({
    "runtime": runtime,
    "operation": operation,
    "workstream_lifecycle": workstream_row[0] if workstream_row else None,
}, sort_keys=True))
PY
}

assert_crash_state() {
    python3 - "$state_root/host.sqlite" "$workstream_id" <<'PY'
import json
import sqlite3
import sys

database, workstream = sys.argv[1:]
connection = sqlite3.connect(database)
operation = connection.execute(
    "SELECT phase, outcome_json, effect_watermark FROM compound_operations WHERE kind = 'start' ORDER BY rowid DESC LIMIT 1"
).fetchone()
if operation is None:
    raise SystemExit("missing durable OpenCode Start journal")
phase, outcome, watermark = operation
if phase != "external_effect_started" or outcome is not None:
    raise SystemExit(f"crash changed Start journal unexpectedly: {operation}")
plan = json.loads(watermark)
if plan.get("provider") != "opencode" or plan.get("native_session_id") is not None:
    raise SystemExit(f"Start journal retained an unexpected provider effect: {plan}")
runtime = connection.execute(
    "SELECT lifecycle, provider_pid, process_birth FROM runtimes WHERE workstream_id = ?", (workstream,)
).fetchone()
if runtime != ("starting", None, None):
    raise SystemExit(f"crash altered Runtime before fail-closed retry: {runtime}")
if connection.execute(
    "SELECT COUNT(*) FROM provider_bindings WHERE runtime_id IN (SELECT runtime_id FROM runtimes WHERE workstream_id = ?)",
    (workstream,),
).fetchone()[0] != 0:
    raise SystemExit("crash unexpectedly created a provider binding")
if connection.execute("SELECT COUNT(*) FROM opencode_runtime_handles").fetchone()[0] != 0:
    raise SystemExit("crash unexpectedly left an OpenCode handle")
PY
}

assert_recovery_state() {
    python3 - "$state_root/host.sqlite" "$workstream_id" <<'PY'
import sqlite3
import sys

database, workstream = sys.argv[1:]
connection = sqlite3.connect(database)
runtime = connection.execute(
    "SELECT lifecycle FROM runtimes WHERE workstream_id = ?", (workstream,)
).fetchone()
workstream_row = connection.execute(
    "SELECT lifecycle FROM workstreams WHERE workstream_id = ?", (workstream,)
).fetchone()
attention = connection.execute(
    "SELECT recovery_unseen_since_revision FROM attention_states WHERE workstream_id = ?", (workstream,)
).fetchone()
if runtime != ("unknown",) or workstream_row != ("recovery_required",) or attention is None or attention[0] is None:
    raise SystemExit(f"retry did not fail closed into recovery: runtime={runtime} workstream={workstream_row} attention={attention}")
PY
}

assert_fake_counts() {
    python3 - "$FAKE_OPENCODE_DB" <<'PY'
import json
import sys
from pathlib import Path

state = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
if state.get("post_count") != 1:
    raise SystemExit(f"provider POST count was not exactly one: {state}")
if state.get("sessions"):
    raise SystemExit(f"provider created a session despite the crash gate: {state}")
if state.get("post_received") is not True:
    raise SystemExit(f"provider POST gate was not reached: {state}")
PY
}

cleanup() {
    local status=$?
    local cleanup_failed=0
    trap - EXIT
    set +e
    if [[ -z "$helper_pid" && -n "$helper_record" && -f "$helper_record" ]]; then
        IFS='|' read -r helper_pid helper_birth helper_group helper_session descendant_pid descendant_birth guardian_pid < <(
            python3 - "$helper_record" <<'PY'
import json
import sys

try:
    value = json.load(open(sys.argv[1], encoding="utf-8"))
    helper = value["helper"]
    descendant = value["descendant"]
    print("|".join(str(item) for item in (
        helper["pid"], helper["birth"], helper["pgrp"], helper["session"],
        descendant["pid"], descendant["birth"], value["guardian_pid"],
    )))
except (KeyError, OSError, TypeError, ValueError, json.JSONDecodeError):
    print("||||||")
PY
        )
        guardian_birth="$(process_birth "$guardian_pid" 2>/dev/null || true)"
    fi
    safe_kill_exact "$action_pid" "$action_birth"
    safe_kill_exact "$guardian_pid" "$guardian_birth"
    safe_kill_exact "$helper_pid" "$helper_birth"
    safe_kill_exact "$descendant_pid" "$descendant_birth"
    safe_kill_exact "$sentinel_pid" "$sentinel_birth"
    assert_process_gone action "$action_pid" "$action_birth" || cleanup_failed=1
    assert_process_gone guardian "$guardian_pid" "$guardian_birth" || cleanup_failed=1
    assert_process_gone helper "$helper_pid" "$helper_birth" || cleanup_failed=1
    assert_process_gone helper-descendant "$descendant_pid" "$descendant_birth" || cleanup_failed=1
    assert_process_gone sentinel "$sentinel_pid" "$sentinel_birth" || cleanup_failed=1
    if [[ -n "$helper_group" && -n "$helper_session" ]]; then
        assert_group_empty "$helper_group" "$helper_session" || cleanup_failed=1
    fi
    if [[ -n "$state_root" && -d "$state_root/run" ]]; then
        while IFS= read -r socket; do
            env -u TMUX tmux -S "$socket" kill-server >/dev/null 2>&1 || true
        done < <(find "$state_root/run" -type s -name tmux.sock -print 2>/dev/null)
    fi
    if ((cleanup_failed == 0)); then
        rm -rf -- "$task_root"
    else
        echo "refusing to remove disposable root while an owned process remains" >&2
        status=1
    fi
    return "$status"
}
trap cleanup EXIT

cargo build --quiet
wsnav_bin="$workspace_root/target/debug/wsnav"
fake_bin="$task_root/bin"
state_root="$task_root/state"
repository="$task_root/repository"
fake_server="$task_root/fake-opencode.py"
fake_db="$task_root/opencode-state.json"
post_release="$task_root/release-post"
helper_record="$task_root/helper-record.json"
mkdir -p "$fake_bin" "$repository"

cat >"$fake_server" <<'PY'
#!/usr/bin/env python3
import json
import os
import signal
import subprocess
import sys
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import urlparse

DB = Path(os.environ["FAKE_OPENCODE_DB"])
RELEASE = Path(os.environ["FAKE_OPENCODE_RELEASE"])
HELPER_RECORD = Path(os.environ["FAKE_OPENCODE_HELPER_RECORD"])

def load():
    if not DB.exists():
        return {"post_count": 0, "post_received": False, "sessions": []}
    return json.loads(DB.read_text(encoding="utf-8"))

def save(value):
    temporary = DB.with_suffix(DB.suffix + ".tmp")
    temporary.write_text(json.dumps(value, sort_keys=True), encoding="utf-8")
    os.replace(temporary, DB)

def proc_identity(pid):
    stat = Path(f"/proc/{pid}/stat").read_text(encoding="utf-8")
    close_paren = stat.rfind(")")
    fields = stat[close_paren + 2 :].split()
    return {"pid": pid, "birth": fields[19], "ppid": int(fields[1]), "pgrp": int(fields[2]), "session": int(fields[3])}

def write_helper_record(descendant):
    value = {"helper": proc_identity(os.getpid()), "descendant": proc_identity(descendant), "guardian_pid": os.getppid()}
    temporary = HELPER_RECORD.with_suffix(HELPER_RECORD.suffix + ".tmp")
    temporary.write_text(json.dumps(value, sort_keys=True), encoding="utf-8")
    os.replace(temporary, HELPER_RECORD)

class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *_args):
        return

    def send_json(self, value):
        body = json.dumps(value, separators=(",", ":")).encode("utf-8")
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Connection", "close")
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):  # noqa: N802
        path = urlparse(self.path).path
        if path == "/global/health":
            self.send_json({"healthy": True, "version": "guardian-fixture-build"})
            return
        if path == "/session/status":
            self.send_json({})
            return
        if path.startswith("/session/") and path.endswith("/message"):
            self.send_json([])
            return
        if path.startswith("/session/"):
            session = path.rsplit("/", 1)[-1]
            if session not in load().get("sessions", []):
                self.send_error(404)
                return
            self.send_json({"id": session, "directory": str(Path.cwd())})
            return
        if path == "/global/event":
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.send_header("Connection", "keep-alive")
            self.end_headers()
            while True:
                time.sleep(1)
        else:
            self.send_error(404)

    def do_POST(self):  # noqa: N802
        if urlparse(self.path).path != "/session":
            self.send_error(404)
            return
        length = int(self.headers.get("Content-Length", "0"))
        self.rfile.read(length)
        state = load()
        state["post_count"] = state.get("post_count", 0) + 1
        state["post_received"] = True
        save(state)
        while not RELEASE.exists():
            time.sleep(0.02)
        state = load()
        session = f"guardian-fixture-session-{state['post_count']}"
        state.setdefault("sessions", []).append(session)
        save(state)
        self.send_json({"id": session})

def main():
    if sys.argv[1:] == ["--version"]:
        print("opencode guardian fixture")
        return 0
    args = sys.argv[1:]
    if not args or args[0] != "serve":
        return 2
    signal.signal(signal.SIGHUP, signal.SIG_IGN)
    signal.signal(signal.SIGTERM, signal.SIG_IGN)
    descendant = subprocess.Popen([sys.executable, "-c", "import signal,time; signal.signal(signal.SIGHUP, signal.SIG_IGN); signal.signal(signal.SIGTERM, signal.SIG_IGN); signal.signal(signal.SIGINT, signal.SIG_IGN); time.sleep(300)"])
    for _ in range(100):
        try:
            write_helper_record(descendant.pid)
            break
        except (FileNotFoundError, IndexError, OSError):
            time.sleep(0.01)
    else:
        return 3
    port = int(args[args.index("--port") + 1])
    server = ThreadingHTTPServer(("127.0.0.1", port), Handler)
    server.daemon_threads = True
    server.serve_forever()
    return 0

raise SystemExit(main())
PY
chmod 700 "$fake_server"

cat >"$fake_bin/opencode" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
exec python3 "$FAKE_OPENCODE_SERVER" "$@"
SH
chmod 700 "$fake_bin/opencode"

git -C "$repository" init -q -b main
git -C "$repository" config user.name wsnav-acceptance
git -C "$repository" config user.email wsnav@example.test
printf 'base\n' >"$repository/README"
git -C "$repository" add README
git -C "$repository" commit -qm initial

export FAKE_OPENCODE_DB="$fake_db"
export FAKE_OPENCODE_RELEASE="$post_release"
export FAKE_OPENCODE_HELPER_RECORD="$helper_record"
export FAKE_OPENCODE_SERVER="$fake_server"
export PATH="$fake_bin:$workspace_root/target/debug:$PATH"

setsid python3 -c 'import signal,time; signal.signal(signal.SIGHUP, signal.SIG_IGN); signal.signal(signal.SIGTERM, signal.SIG_IGN); time.sleep(300)' \
    </dev/null >"$task_root/sentinel.out" 2>&1 &
sentinel_pid=$!
sentinel_birth="$(process_birth "$sentinel_pid")"
capture_process sentinel "$sentinel_pid"

registration="$("$wsnav_bin" --state-root "$state_root" register --provider opencode "$repository")"
workstream_id="$(printf '%s\n' "$registration" | awk '{print $NF}')"

"$wsnav_bin" --state-root "$state_root" start "$workstream_id" >"$task_root/start.out" 2>&1 &
action_pid=$!
action_birth="$(process_birth "$action_pid")"
capture_process action "$action_pid"

boundary_seen=0
post_seen=0
for _ in {1..300}; do
    assert_process_live start-action "$action_pid" "$action_birth"
    current_journal="$(journal_state 2>/dev/null || true)"
    if [[ -n "$current_journal" ]] && python3 - "$current_journal" <<'PY'
import json
import sys

value = json.loads(sys.argv[1])
operation = value.get("operation")
if operation is None or operation[1] != "external_effect_started":
    raise SystemExit(1)
PY
    then
        boundary_seen=1
    fi
    if [[ -f "$helper_record" && "$boundary_seen" == 1 ]]; then
        helper_pid="$(python3 - "$helper_record" <<'PY'
import json
import sys
print(json.load(open(sys.argv[1], encoding="utf-8"))["helper"]["pid"])
PY
        )"
        helper_birth="$(python3 - "$helper_record" <<'PY'
import json
import sys
print(json.load(open(sys.argv[1], encoding="utf-8"))["helper"]["birth"])
PY
        )"
        descendant_pid="$(python3 - "$helper_record" <<'PY'
import json
import sys
print(json.load(open(sys.argv[1], encoding="utf-8"))["descendant"]["pid"])
PY
        )"
        descendant_birth="$(python3 - "$helper_record" <<'PY'
import json
import sys
print(json.load(open(sys.argv[1], encoding="utf-8"))["descendant"]["birth"])
PY
        )"
        guardian_pid="$(python3 - "$helper_record" <<'PY'
import json
import sys
print(json.load(open(sys.argv[1], encoding="utf-8"))["guardian_pid"])
PY
        )"
        guardian_birth="$(process_birth "$guardian_pid" 2>/dev/null || true)"
        if [[ -n "$guardian_birth" ]] && assert_process_live helper "$helper_pid" "$helper_birth" \
            && assert_process_live helper-descendant "$descendant_pid" "$descendant_birth"; then
            capture_process helper "$helper_pid"
            capture_process descendant "$descendant_pid"
            capture_process guardian "$guardian_pid"
            helper_group="$(python3 - "$task_root/process-helper.json" <<'PY'
import json
import sys
print(json.load(open(sys.argv[1], encoding="utf-8"))["pgrp"])
PY
            )"
            helper_session="$(python3 - "$task_root/process-helper.json" <<'PY'
import json
import sys
print(json.load(open(sys.argv[1], encoding="utf-8"))["session"])
PY
            )"
            break
        fi
    fi
    sleep 0.1
done
[[ "$boundary_seen" == 1 ]]
[[ -n "$helper_pid" && -n "$descendant_pid" && -n "$guardian_pid" && -n "$helper_group" && -n "$helper_session" ]]
assert_process_live guardian "$guardian_pid" "$guardian_birth"
assert_process_relations

for _ in {1..200}; do
    if [[ "$(fake_state)" == *'"post_received": true'* ]]; then
        post_seen=1
        break
    fi
    sleep 0.05
done
[[ "$post_seen" == 1 ]]
assert_fake_counts
assert_process_live start-action "$action_pid" "$action_birth"
assert_process_live helper "$helper_pid" "$helper_birth"
assert_process_live helper-descendant "$descendant_pid" "$descendant_birth"

# The action is blocked in the gated POST. Revalidate its birth token, then
# kill only this PID; never signal its process group because the guardian owns
# the helper group.
[[ "$(process_birth "$action_pid" 2>/dev/null || true)" == "$action_birth" ]]
kill -KILL "$action_pid"
if wait "$action_pid"; then
    action_status=0
else
    action_status=$?
fi
[[ "$action_status" == 137 ]]
action_pid=""

assert_process_gone guardian "$guardian_pid" "$guardian_birth"
assert_process_gone helper "$helper_pid" "$helper_birth"
assert_process_gone helper-descendant "$descendant_pid" "$descendant_birth"
assert_group_empty "$helper_group" "$helper_session"
assert_fake_counts
assert_crash_state

if "$wsnav_bin" --state-root "$state_root" start "$workstream_id" >"$task_root/retry-one.out" 2>&1; then
    echo "Start unexpectedly retried an unresolved OpenCode creation" >&2
    exit 1
fi
grep -E -i 'recovery|unavailable|unknown' "$task_root/retry-one.out" >/dev/null
assert_recovery_state
assert_fake_counts

if "$wsnav_bin" --state-root "$state_root" start "$workstream_id" >"$task_root/retry-two.out" 2>&1; then
    echo "second Start unexpectedly retried a fail-closed creation" >&2
    exit 1
fi
assert_recovery_state
assert_fake_counts

status_output="$("$wsnav_bin" --state-root "$state_root" status "$workstream_id")"
grep -F 'workstream: RecoveryRequired' <<<"$status_output" >/dev/null
grep -F 'lifecycle: Unknown' <<<"$status_output" >/dev/null
grep -F 'recovery attention: unseen' <<<"$status_output" >/dev/null

assert_process_live unrelated-sentinel "$sentinel_pid" "$sentinel_birth"
ordinary_tmux_after="$(env -u TMUX tmux list-sessions -F '#{session_name}:#{session_created}:#{session_windows}' -O name 2>/dev/null || true)"
[[ "$ordinary_tmux_before" == "$ordinary_tmux_after" ]]
[[ -z "$(find "$state_root/run" -type s -name tmux.sock -print -quit 2>/dev/null)" ]]

workstream_id=""
trap - EXIT
cleanup
[[ ! -e "$task_root" ]]
printf 'D8.2 disposable OpenCode creation-guardian crash acceptance passed\n'
