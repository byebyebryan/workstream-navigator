#!/usr/bin/env bash
# Disposable D8.1/D8.2 OpenCode/private-tmux acceptance. It never uses
# ordinary provider state or the user's default tmux server.
set -euo pipefail

workspace_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
task_root="$(mktemp -d)"
ordinary_tmux_before="$(env -u TMUX tmux list-sessions -F '#{session_name}:#{session_created}:#{session_windows}' -O name 2>/dev/null || true)"
wsnav_bin=""
state_root=""
workstream_id=""
fork_workstream_id=""
attach_socket=""

cleanup() {
    if [[ -n "$wsnav_bin" && -n "$state_root" && -n "$workstream_id" ]]; then
        [[ -z "$fork_workstream_id" ]] || "$wsnav_bin" --state-root "$state_root" park "$fork_workstream_id" >/dev/null 2>&1 || true
        "$wsnav_bin" --state-root "$state_root" park "$workstream_id" >/dev/null 2>&1 || true
    fi
    if [[ -n "$attach_socket" ]]; then
        env -u TMUX tmux -S "$attach_socket" kill-server >/dev/null 2>&1 || true
    fi
    if [[ -n "$state_root" && -d "$state_root/run" ]]; then
        while IFS= read -r socket; do
            env -u TMUX tmux -S "$socket" kill-server >/dev/null 2>&1 || true
        done < <(find "$state_root/run" -type s -name tmux.sock -print 2>/dev/null)
    fi
    rm -rf -- "$task_root"
}
trap cleanup EXIT

cargo build --quiet
wsnav_bin="$workspace_root/target/debug/wsnav"
fake_bin="$task_root/bin"
state_root="$task_root/state"
repository="$task_root/repository"
fake_db="$task_root/opencode-db.json"
fake_server="$task_root/fake-opencode.py"
mkdir -p "$fake_bin" "$repository"

cat >"$fake_server" <<'PY'
#!/usr/bin/env python3
import json
import os
import signal
import sys
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import urlparse


DB = Path(os.environ["FAKE_OPENCODE_DB"])


def load_db():
    if not DB.exists():
        return {
            "counter": 0,
            "sessions": [],
            "event_sent": False,
            "status": {},
            "fork_requests": [],
            "drop_next_fork_response": False,
            "event_stream_connected": False,
        }
    return json.loads(DB.read_text(encoding="utf-8"))


def save_db(value):
    DB.write_text(json.dumps(value), encoding="utf-8")


def chunk(stream, payload):
    stream.write(f"{len(payload):X}\r\n".encode("ascii"))
    stream.write(payload)
    stream.write(b"\r\n")
    stream.flush()


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *_args):
        return

    def json_response(self, value):
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
            self.json_response({"healthy": True, "version": "99.7.3"})
            return
        if path == "/session/status":
            state = load_db()
            statuses = {
                session: {"type": state.get("status", {}).get(session, "idle")}
                for session in state["sessions"]
                if state.get("status", {}).get(session, "idle") != "idle"
            }
            statuses["child-session"] = {"type": "busy"}
            statuses["unrelated-session"] = {"type": "busy"}
            self.json_response(statuses)
            return
        if path.endswith("/message") and path.startswith("/session/"):
            self.json_response([])
            return
        if path.startswith("/session/"):
            state = load_db()
            session = path.rsplit("/", 1)[-1]
            if session not in state["sessions"]:
                self.send_error(404)
                return
            self.json_response(
                {
                    "id": session,
                    "directory": state.get("directory", str(Path.cwd())),
                }
            )
            return
        if path == "/global/event":
            time.sleep(0.25)
            state = load_db()
            state["event_stream_connected"] = True
            save_db(state)
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.send_header("Transfer-Encoding", "chunked")
            self.send_header("Connection", "keep-alive")
            self.end_headers()
            state = load_db()
            if not state["event_sent"]:
                state["event_sent"] = True
                save_db(state)
                session = state["sessions"][-1]
                child = {
                    "payload": {
                        "type": "session.status",
                        "properties": {"sessionID": "child-session", "status": {"type": "busy"}},
                    }
                }
                busy = {
                    "payload": {
                        "type": "session.status",
                        "properties": {
                            "sessionID": session,
                            "status": {"type": "busy"},
                        }
                    }
                }
                candidate = {
                    "payload": {
                        "type": "message.updated",
                        "properties": {
                            "sessionID": session,
                            "info": {
                                "id": "completed-message",
                                "sessionID": session,
                                "role": "assistant",
                                "finish": "stop",
                                "time": {"completed": 1},
                            },
                        }
                    }
                }
                state = load_db()
                state.setdefault("status", {})[session] = "busy"
                save_db(state)
                idle_status = {
                    "payload": {
                        "type": "session.status",
                        "properties": {
                            "sessionID": session,
                            "status": {"type": "idle"},
                        }
                    }
                }
                idle = {
                    "payload": {
                        "type": "session.idle",
                        "properties": {"sessionID": session},
                    }
                }
                state = load_db()
                state.setdefault("status", {})[session] = "idle"
                save_db(state)
                for value in (child, busy, candidate, busy, idle_status, idle):
                    chunk(self.wfile, f"data: {json.dumps(value)}\n\n".encode("utf-8"))
            while True:
                time.sleep(1)
        else:
            self.send_error(404)

    def do_POST(self):  # noqa: N802
        path = urlparse(self.path).path
        length = int(self.headers.get("Content-Length", "0"))
        raw_body = self.rfile.read(length)
        if path.startswith("/session/") and path.endswith("/fork"):
            source = path.removeprefix("/session/").removesuffix("/fork")
            state = load_db()
            if source not in state["sessions"]:
                self.send_error(404)
                return
            try:
                body = json.loads(raw_body)
            except json.JSONDecodeError:
                self.send_error(400)
                return
            if body != {"messageID": "completed-message"}:
                self.send_error(400)
                return
            state["counter"] += 1
            destination = f"fake-session-{state['counter']}"
            state["sessions"].append(destination)
            state.setdefault("status", {})[destination] = "idle"
            state.setdefault("fork_requests", []).append(
                {"source": source, "messageID": body["messageID"]}
            )
            drop_response = state.get("drop_next_fork_response", False)
            state["drop_next_fork_response"] = False
            save_db(state)
            if drop_response:
                self.close_connection = True
                return
            self.json_response({"id": destination})
            return
        if path != "/session":
            self.send_error(404)
            return
        state = load_db()
        state["counter"] += 1
        session = f"fake-session-{state['counter']}"
        state["sessions"].append(session)
        state["directory"] = str(Path.cwd())
        state.setdefault("status", {})[session] = "idle"
        save_db(state)
        self.json_response({"id": session})


def main():
    if sys.argv[1:] == ["--version"]:
        print("opencode 99.7.3")
        return 0
    args = sys.argv[1:]
    if not args:
        return 2
    session = None if args[0] == "serve" else args[args.index("--session") + 1]
    port = int(args[args.index("--port") + 1])
    if session is not None and session not in load_db()["sessions"]:
        return 3
    # A provider is not required to exit merely because tmux closes its PTY.
    # Deliberately survive that boundary so the acceptance proves WSNav's
    # persisted PID-plus-birth cleanup authority instead of tmux behavior.
    signal.signal(signal.SIGHUP, signal.SIG_IGN)
    server = ThreadingHTTPServer(("127.0.0.1", port), Handler)
    server.daemon_threads = True
    if session is not None:
        print("FAKE_OPENCODE_NATIVE_SURFACE", flush=True)
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
git -C "$repository" config user.name wsnav-test
git -C "$repository" config user.email wsnav@example.test
printf 'base\n' >"$repository/README"
git -C "$repository" add README
git -C "$repository" commit -qm initial

export FAKE_OPENCODE_DB="$fake_db"
export FAKE_OPENCODE_SERVER="$fake_server"
export PATH="$fake_bin:$workspace_root/target/debug:$PATH"

handle_row() {
    local target_workstream_id="${1:-$workstream_id}"
    python3 - "$state_root/host.sqlite" "$target_workstream_id" <<'PY'
import sqlite3
import sys

connection = sqlite3.connect(sys.argv[1])
row = connection.execute(
    """SELECT r.provider_pid, r.process_birth,
                     h.observer_pid, h.observer_birth, h.observer_status,
                     h.endpoint_port, h.runtime_generation,
                     b.native_session_id
          FROM opencode_runtime_handles h
          JOIN runtimes r ON r.runtime_id = h.runtime_id
          JOIN provider_bindings b ON b.runtime_id = h.runtime_id
          WHERE r.workstream_id = ?""",
    (sys.argv[2],),
).fetchone()
if row is None:
    raise SystemExit("missing OpenCode runtime handle")
print("|".join(str(value) for value in row))
PY
}

assert_pid_gone() {
    python3 - "$1" <<'PY'
from pathlib import Path
import sys

stat = Path(f"/proc/{sys.argv[1]}/stat")
if stat.exists():
    state = stat.read_text(encoding="utf-8").rsplit(")", 1)[-1].strip().split()[0]
    if state != "Z":
        raise SystemExit("observer process still live")
PY
}

assert_process_identity_gone() {
    python3 - "$1" "$2" <<'PY'
from pathlib import Path
import sys

pid, expected_birth = sys.argv[1:]
try:
    stat = Path(f"/proc/{pid}/stat").read_text()
    current_birth = stat[stat.rfind(")") + 2:].split()[19]
except FileNotFoundError:
    raise SystemExit(0)
except (IndexError, OSError) as error:
    raise SystemExit(f"provider process identity probe was ambiguous: {pid}") from error
if current_birth == expected_birth:
    raise SystemExit(f"exact provider process survived cleanup: {pid}")
PY
}

assert_port_closed() {
    python3 - "$1" <<'PY'
import socket
import sys

try:
    socket.create_connection(("127.0.0.1", int(sys.argv[1])), timeout=0.5)
except OSError:
    pass
else:
    raise SystemExit("OpenCode endpoint still listening")
PY
}

assert_final_state() {
    python3 - "$state_root/host.sqlite" "$workstream_id" <<'PY'
import sqlite3
import sys

connection = sqlite3.connect(sys.argv[1])
runtime = connection.execute(
    "SELECT lifecycle FROM runtimes WHERE workstream_id = ?", (sys.argv[2],)
).fetchone()
handles = connection.execute("SELECT COUNT(*) FROM opencode_runtime_handles").fetchone()[0]
if runtime != ("stopped",) or handles != 0:
    raise SystemExit("disposable OpenCode state was not parked and cleaned")
PY
}

registration="$($wsnav_bin --state-root "$state_root" register --provider opencode "$repository")"
workstream_id="${registration##* }"
"$wsnav_bin" --state-root "$state_root" start "$workstream_id"
python3 - "$fake_db" <<'PY'
import json
import sys

if not json.loads(open(sys.argv[1], encoding="utf-8").read()).get(
    "event_stream_connected", False
):
    raise SystemExit("observer became ready before its SSE stream")
PY
sleep 1
first_status="$($wsnav_bin --state-root "$state_root" status "$workstream_id")"
grep -F 'lifecycle: Attention' <<<"$first_status" >/dev/null
grep -F 'private runtime: live' <<<"$first_status" >/dev/null
grep -F 'provider binding: bound' <<<"$first_status" >/dev/null
IFS='|' read -r first_provider_pid first_provider_birth first_observer_pid first_observer_birth first_observer_status first_port first_generation first_session <<<"$(handle_row)"
[[ "$first_observer_status" == "ready" ]]
[[ -n "$first_provider_pid" && -n "$first_provider_birth" ]]
[[ -n "$first_observer_birth" && -n "$first_generation" && -n "$first_session" ]]

runtime_socket="$(find "$state_root/run" -type s -name tmux.sock -print -quit)"
runtime_session="$(env -u TMUX tmux -S "$runtime_socket" list-sessions -F '#{session_name}' | head -n 1)"
attach_socket="$task_root/attach.sock"
attach_session="wsnav-opencode-attach"
attach_done="$task_root/attach.done"
printf -v attach_command '%q --state-root %q attach %q && touch %q' \
    "$wsnav_bin" "$state_root" "$workstream_id" "$attach_done"
env -u TMUX tmux -u -f /dev/null -S "$attach_socket" \
    new-session -d -s "$attach_session" -c "$repository" "$attach_command"
surface_seen=0
attached_client=""
for _ in {1..100}; do
    attached_clients="$(env -u TMUX tmux -S "$runtime_socket" list-clients -F '#{client_session}' 2>/dev/null || true)"
    if grep -Fx "$runtime_session" <<<"$attached_clients" >/dev/null 2>&1; then
        attached_client="$(env -u TMUX tmux -S "$runtime_socket" list-clients -F '#{client_tty}' | head -n 1)"
        native_surface="$(env -u TMUX tmux -S "$runtime_socket" capture-pane -p -J -t "$runtime_session:0.0" 2>/dev/null || true)"
        driver_surface="$(env -u TMUX tmux -S "$attach_socket" capture-pane -p -J -t "$attach_session:0.0" 2>/dev/null || true)"
        if grep -F 'FAKE_OPENCODE_NATIVE_SURFACE' <<<"$native_surface" >/dev/null \
            && grep -F 'FAKE_OPENCODE_NATIVE_SURFACE' <<<"$driver_surface" >/dev/null; then
            surface_seen=1
            break
        fi
    fi
    sleep 0.1
done
test "$surface_seen" -eq 1
env -u TMUX tmux -S "$runtime_socket" detach-client -t "$attached_client"
for _ in {1..100}; do
    [[ -f "$attach_done" ]] && break
    sleep 0.1
done
test -f "$attach_done"
env -u TMUX tmux -S "$attach_socket" kill-server >/dev/null 2>&1 || true
attach_socket=""

# A dead observer must make the next start/validation refuse adoption and
# durably mark only its exact handle Unknown. Parking still owns and cleans
# the provider Runtime and endpoint without signaling the reused PID.
kill -KILL "$first_observer_pid"
assert_pid_gone "$first_observer_pid"
if "$wsnav_bin" --state-root "$state_root" start "$workstream_id" \
    >"$task_root/dead-observer-start.out" 2>&1; then
    echo "start unexpectedly adopted a Runtime with a dead observer" >&2
    exit 1
fi
IFS='|' read -r dead_provider_pid dead_provider_birth dead_observer_pid dead_observer_birth dead_observer_status _dead_port _dead_generation _dead_session <<<"$(handle_row)"
[[ "$dead_provider_pid" == "$first_provider_pid" ]]
[[ "$dead_provider_birth" == "$first_provider_birth" ]]
[[ "$dead_observer_pid" == "$first_observer_pid" ]]
[[ "$dead_observer_birth" == "$first_observer_birth" ]]
[[ "$dead_observer_status" == "unknown" ]]

"$wsnav_bin" --state-root "$state_root" park "$workstream_id"
assert_process_identity_gone "$first_provider_pid" "$first_provider_birth"
assert_port_closed "$first_port"
"$wsnav_bin" --state-root "$state_root" start "$workstream_id"
sleep 1
resume_status="$($wsnav_bin --state-root "$state_root" status "$workstream_id")"
grep -F 'lifecycle: Idle' <<<"$resume_status" >/dev/null
grep -F 'private runtime: live' <<<"$resume_status" >/dev/null
IFS='|' read -r second_provider_pid second_provider_birth second_observer_pid second_observer_birth second_observer_status second_port second_generation second_session <<<"$(handle_row)"
[[ "$second_observer_status" == "ready" ]]
[[ -n "$second_provider_pid" && -n "$second_provider_birth" ]]
[[ "$second_session" == "$first_session" ]]
[[ "$second_generation" != "$first_generation" && "$second_port" != "$first_port" ]]
[[ "$second_observer_pid" != "$first_observer_pid" && "$second_observer_birth" != "$first_observer_birth" ]]

# Conclusive loss of the exact private Runtime keeps the bound root session,
# stops/replaces only its old observer and handle, and resumes it under a new
# generation and endpoint. No provider picker, discovery, or blank session is
# involved.
runtime_socket="$(find "$state_root/run" -type s -name tmux.sock -print -quit)"
env -u TMUX tmux -S "$runtime_socket" kill-server
sleep 1
lost_status="$($wsnav_bin --state-root "$state_root" status "$workstream_id")"
grep -F 'workstream: RecoveryRequired' <<<"$lost_status" >/dev/null
grep -F 'lifecycle: Unknown' <<<"$lost_status" >/dev/null
grep -F 'recovery attention: unseen' <<<"$lost_status" >/dev/null
"$wsnav_bin" --state-root "$state_root" recover "$workstream_id"
sleep 1
recovered_status="$($wsnav_bin --state-root "$state_root" status "$workstream_id")"
grep -F 'workstream: Open' <<<"$recovered_status" >/dev/null
grep -F 'lifecycle: Idle' <<<"$recovered_status" >/dev/null
grep -F 'recovery attention: none' <<<"$recovered_status" >/dev/null
IFS='|' read -r recovered_provider_pid recovered_provider_birth recovered_observer_pid recovered_observer_birth recovered_observer_status recovered_port recovered_generation recovered_session <<<"$(handle_row)"
[[ "$recovered_observer_status" == "ready" ]]
[[ -n "$recovered_provider_pid" && -n "$recovered_provider_birth" ]]
[[ -n "$recovered_observer_birth" ]]
[[ "$recovered_session" == "$second_session" ]]
[[ "$recovered_generation" != "$second_generation" && "$recovered_port" != "$second_port" ]]
assert_pid_gone "$second_observer_pid"
assert_process_identity_gone "$second_provider_pid" "$second_provider_birth"
assert_port_closed "$second_port"

# The provider fork is issued through the exact live endpoint at the observer's
# settled messageID. The destination becomes a distinct same-provider
# Workstream and starts through the ordinary exact-resume path.
forked="$($wsnav_bin --state-root "$state_root" fork-workstream "$workstream_id")"
fork_workstream_id="${forked##* }"
sleep 1
IFS='|' read -r fork_provider_pid fork_provider_birth fork_observer_pid fork_observer_birth fork_observer_status fork_port fork_generation fork_session <<<"$(handle_row "$fork_workstream_id")"
[[ "$fork_observer_status" == "ready" ]]
[[ -n "$fork_provider_pid" && -n "$fork_provider_birth" ]]
[[ -n "$fork_observer_birth" && -n "$fork_generation" ]]
[[ "$fork_session" != "$recovered_session" ]]
python3 - "$state_root/host.sqlite" "$fake_db" "$workstream_id" "$fork_workstream_id" <<'PY'
import json
import sqlite3
import sys

database, fake_database, source, destination = sys.argv[1:]
connection = sqlite3.connect(database)
rows = connection.execute(
    "SELECT workstream_id, provider FROM workstreams WHERE workstream_id IN (?, ?)",
    (source, destination),
).fetchall()
if sorted(rows) != sorted([(source, "opencode"), (destination, "opencode")]):
    raise SystemExit("OpenCode Fork did not retain same-provider identity")
operation = connection.execute(
    "SELECT phase FROM compound_operations ORDER BY rowid DESC LIMIT 1"
).fetchone()
if operation != ("committed",):
    raise SystemExit("OpenCode Fork did not commit exactly")
provider = json.loads(open(fake_database, encoding="utf-8").read())
if provider.get("fork_requests") != [
    {"source": provider["sessions"][0], "messageID": "completed-message"}
]:
    raise SystemExit("OpenCode Fork did not use the exact settled boundary")
PY

"$wsnav_bin" --state-root "$state_root" park "$fork_workstream_id"
assert_pid_gone "$fork_observer_pid"
assert_process_identity_gone "$fork_provider_pid" "$fork_provider_birth"
assert_port_closed "$fork_port"
fork_workstream_id=""

# A response lost after the provider creates a destination is terminal and
# never creates or adopts a WSNav Workstream. The unmanaged disposable provider
# session is removed with the rest of the fake provider root at cleanup.
python3 - "$fake_db" <<'PY'
import json
import sys

path = sys.argv[1]
state = json.loads(open(path, encoding="utf-8").read())
state["drop_next_fork_response"] = True
open(path, "w", encoding="utf-8").write(json.dumps(state))
PY
if "$wsnav_bin" --state-root "$state_root" fork-workstream "$workstream_id" \
    >"$task_root/lost-fork.out" 2>&1; then
    echo "OpenCode Fork unexpectedly accepted a lost response" >&2
    exit 1
fi
grep -F 'external effect is unknown' "$task_root/lost-fork.out" >/dev/null
python3 - "$state_root/host.sqlite" "$fake_db" <<'PY'
import json
import sqlite3
import sys

connection = sqlite3.connect(sys.argv[1])
phase, outcome = connection.execute(
    "SELECT phase, outcome_json FROM compound_operations ORDER BY rowid DESC LIMIT 1"
).fetchone()
if phase != "failed" or json.loads(outcome) != {"code": "external_effect_unknown"}:
    raise SystemExit("lost OpenCode Fork response was not terminally bounded")
if connection.execute("SELECT COUNT(*) FROM workstreams").fetchone()[0] != 2:
    raise SystemExit("lost OpenCode Fork response created a WSNav destination")
provider = json.loads(open(sys.argv[2], encoding="utf-8").read())
if len(provider.get("fork_requests", [])) != 2:
    raise SystemExit("lost OpenCode Fork was not issued exactly once")
PY

"$wsnav_bin" --state-root "$state_root" park "$workstream_id"
assert_pid_gone "$recovered_observer_pid"
assert_process_identity_gone "$recovered_provider_pid" "$recovered_provider_birth"
assert_port_closed "$recovered_port"
assert_final_state
test -z "$(find "$state_root/run" -type s -name tmux.sock -print -quit 2>/dev/null)"
workstream_id=""

ordinary_tmux_after="$(env -u TMUX tmux list-sessions -F '#{session_name}:#{session_created}:#{session_windows}' -O name 2>/dev/null || true)"
[[ "$ordinary_tmux_before" == "$ordinary_tmux_after" ]]
printf 'D8.1/D8.2 disposable fake OpenCode/private-tmux acceptance passed\n'
