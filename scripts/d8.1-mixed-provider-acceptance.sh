#!/usr/bin/env bash
# Disposable mixed-provider D8.1 acceptance. Provider state, homes, and
# private Runtime tmux servers are all rooted below one temporary directory.
set -euo pipefail

workspace_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
task_root="$(mktemp -d)"
ordinary_tmux_before="$(env -u TMUX tmux list-sessions -F '#{session_name}:#{session_created}:#{session_windows}' -O name 2>/dev/null || true)"
wsnav_bin=""
state_root=""
codex_workstream_id=""
opencode_workstream_id=""
codex_runtime_id=""
opencode_runtime_id=""
codex_identity_file=""
opencode_identity_file=""

cleanup() {
    if [[ -n "$wsnav_bin" && -n "$state_root" ]]; then
        [[ -z "$opencode_workstream_id" ]] || "$wsnav_bin" --state-root "$state_root" park "$opencode_workstream_id" >/dev/null 2>&1 || true
        [[ -z "$codex_workstream_id" ]] || "$wsnav_bin" --state-root "$state_root" park "$codex_workstream_id" >/dev/null 2>&1 || true
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
codex_home="$task_root/codex-home"
repository="$task_root/repository"
opencode_db="$task_root/opencode-db.json"
provider_args="$task_root/provider-args.log"
mkdir -p "$fake_bin" "$codex_home" "$repository"

cat >"$fake_bin/codex" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
printf 'codex %s\n' "$*" >>"$FAKE_PROVIDER_ARGS"
if [[ "${1:-}" == "--version" ]]; then printf 'codex fixture 0.1.0\n'; exit 0; fi
if [[ "${1:-}" == "app-server" ]]; then
    while IFS= read -r line; do
        case "$line" in
            *'"id":1'*) printf '%s\n' '{"id":1,"result":{}}' ;;
            *'"id":2'*'thread/read'*) printf '%s\n' '{"id":2,"result":{"thread":{"id":"mixed-codex-session","name":""}}}' ;;
            *'"id":2'*'thread/name/set'*) printf '%s\n' '{"id":2,"result":{}}' ;;
        esac
    done
    exit 0
fi
source_value=startup
if [[ " $* " == *" resume "* ]]; then source_value=resume; fi
printf '{"hook_event_name":"SessionStart","cwd":"%s","session_id":"mixed-codex-session","source":"%s"}' "$PWD" "$source_value" | wsnav --state-root "$FAKE_STATE_ROOT" _hook
printf '{"hook_event_name":"UserPromptSubmit","cwd":"%s","session_id":"mixed-codex-session","turn_id":"mixed-codex-turn"}' "$PWD" | wsnav --state-root "$FAKE_STATE_ROOT" _hook
printf '{"hook_event_name":"Stop","cwd":"%s","session_id":"mixed-codex-session","turn_id":"mixed-codex-turn"}' "$PWD" | wsnav --state-root "$FAKE_STATE_ROOT" _hook
printf 'MIXED_CODEX_NATIVE_SURFACE\n'
sleep 60
SH
chmod 700 "$fake_bin/codex"

cat >"$task_root/fake-opencode.py" <<'PY'
#!/usr/bin/env python3
import json, os, sys, time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import urlparse

DB = Path(os.environ["FAKE_OPENCODE_DB"])
def load():
    return json.loads(DB.read_text()) if DB.exists() else {"counter": 0, "sessions": []}
def save(value):
    DB.write_text(json.dumps(value))
class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    def log_message(self, *_args): pass
    def send_json(self, value):
        body = json.dumps(value, separators=(",", ":")).encode()
        self.send_response(200); self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body))); self.send_header("Connection", "close")
        self.end_headers(); self.wfile.write(body)
    def do_GET(self):
        path = urlparse(self.path).path
        if path == "/global/health": self.send_json({"healthy": True, "version": "1.18.11"})
        elif path == "/session/status": self.send_json({s: {"type": "idle"} for s in load()["sessions"]})
        elif path.startswith("/session/") and path.endswith("/message"): self.send_json([])
        elif path == "/global/event":
            self.send_response(200); self.send_header("Content-Type", "text/event-stream")
            self.send_header("Transfer-Encoding", "chunked"); self.send_header("Connection", "keep-alive"); self.end_headers()
            while True: time.sleep(1)
        else: self.send_error(404)
    def do_POST(self):
        if urlparse(self.path).path != "/session": self.send_error(404); return
        self.rfile.read(int(self.headers.get("Content-Length", "0")))
        state = load(); state["counter"] += 1; session = f"mixed-opencode-session-{state['counter']}"
        state["sessions"].append(session); save(state); self.send_json({"id": session})
if sys.argv[1:] == ["--version"]: print("opencode 1.18.11"); raise SystemExit(0)
args = sys.argv[1:]; port = int(args[args.index("--port") + 1])
session = None if args[0] == "serve" else args[args.index("--session") + 1]
if session is not None:
    if session not in load()["sessions"]: raise SystemExit(3)
    print("MIXED_OPENCODE_NATIVE_SURFACE", flush=True)
ThreadingHTTPServer(("127.0.0.1", port), Handler).serve_forever()
PY
chmod 700 "$task_root/fake-opencode.py"
cat >"$fake_bin/opencode" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
printf 'opencode %s\n' "$*" >>"$FAKE_PROVIDER_ARGS"
exec python3 "$FAKE_OPENCODE_SERVER" "$@"
SH
chmod 700 "$fake_bin/opencode"

git -C "$repository" init -q -b main
git -C "$repository" config user.name wsnav-test
git -C "$repository" config user.email wsnav@example.test
printf 'base\n' >"$repository/README"
git -C "$repository" add README
git -C "$repository" commit -qm initial

export CODEX_HOME="$codex_home"
export FAKE_STATE_ROOT="$state_root"
export FAKE_OPENCODE_DB="$opencode_db"
export FAKE_OPENCODE_SERVER="$task_root/fake-opencode.py"
export FAKE_PROVIDER_ARGS="$provider_args"
export PATH="$fake_bin:$workspace_root/target/debug:$PATH"

# Codex setup is an explicit observer action and creates no Workstream.
"$wsnav_bin" --state-root "$state_root" setup --skip-review
profile_path="$codex_home/wsnav-observer.config.toml"
for hook in session_start user_prompt_submit stop session_end; do
    printf '\n[hooks.state."%s:%s:0:0"]\ntrusted_hash = "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"\n' \
        "$profile_path" "$hook" >>"$profile_path"
done
"$wsnav_bin" --state-root "$state_root" trust-observer

registration="$($wsnav_bin --state-root "$state_root" register --provider codex "$repository")"
codex_workstream_id="${registration##* }"
"$wsnav_bin" --state-root "$state_root" start "$codex_workstream_id"
sleep 1
registration="$($wsnav_bin --state-root "$state_root" new-workstream "$codex_workstream_id" --provider opencode)"
opencode_workstream_id="${registration##* }"
sleep 1

python3 - "$state_root/host.sqlite" "$codex_workstream_id" "$opencode_workstream_id" <<'PY'
import sqlite3, sys
db, codex, opencode = sys.argv[1:]
connection = sqlite3.connect(db)
rows = connection.execute(
    "SELECT workstream_id, provider FROM workstreams WHERE workstream_id IN (?, ?)",
    (codex, opencode),
).fetchall()
if sorted(rows) != sorted([(codex, "codex"), (opencode, "opencode")]):
    raise SystemExit("provider identities crossed or were not persisted")
bindings = connection.execute(
    "SELECT w.workstream_id, b.provider, b.native_session_id FROM workstreams w "
    "JOIN runtimes r ON r.workstream_id = w.workstream_id JOIN provider_bindings b ON b.runtime_id = r.runtime_id "
    "WHERE w.workstream_id IN (?, ?)", (codex, opencode),
).fetchall()
if any(row[0] == codex and row[1] != "codex" for row in bindings): raise SystemExit("Codex binding crossed")
if any(row[0] == opencode and row[1] != "opencode" for row in bindings): raise SystemExit("OpenCode binding crossed")
if {row[2] for row in bindings} != {"mixed-codex-session", "mixed-opencode-session-1"}:
    raise SystemExit("native bindings missing or crossed")
PY

grep -F 'lifecycle: Attention' <<<"$($wsnav_bin --state-root "$state_root" status "$codex_workstream_id")" >/dev/null
grep -F 'private runtime: live' <<<"$($wsnav_bin --state-root "$state_root" status "$opencode_workstream_id")" >/dev/null

runtime_id_for_workstream() {
    python3 - "$state_root/host.sqlite" "$1" <<'PY'
import sqlite3, sys
value = sqlite3.connect(sys.argv[1]).execute(
    "SELECT runtime_id FROM runtimes WHERE workstream_id = ?", (sys.argv[2],)
).fetchone()
if value is None:
    raise SystemExit("runtime is not persisted")
print(value[0])
PY
}

record_runtime_identity() {
    local runtime_id="$1" output_file="$2"
    local socket session pane_pid
    socket="$state_root/run/runtime-$runtime_id/tmux.sock"
    session="$(env -u TMUX tmux -S "$socket" list-sessions -F '#{session_name}' | head -n 1)"
    pane_pid="$(env -u TMUX tmux -S "$socket" display-message -p -t "$session:0.0" '#{pane_pid}')"
    python3 - "$state_root/host.sqlite" "$runtime_id" "$pane_pid" "$socket" >"$output_file" <<'PY'
import json, sqlite3, sys
from pathlib import Path

db, runtime_id, pane_pid_text, socket = sys.argv[1:]
pane_pid = int(pane_pid_text)

def process_birth(pid):
    stat = Path(f"/proc/{pid}/stat").read_text()
    close_paren = stat.rfind(")")
    return stat[close_paren + 2:].split()[19]

connection = sqlite3.connect(db)
runtime = connection.execute(
    "SELECT provider, process_birth FROM runtimes WHERE runtime_id = ?", (runtime_id,)
).fetchone()
if runtime is None:
    raise SystemExit("runtime identity is missing")
provider, recorded_birth = runtime
pane_birth = process_birth(pane_pid)
if recorded_birth != pane_birth:
    raise SystemExit("pane process birth does not match persisted Runtime identity")
identity = {
    "runtime_id": runtime_id,
    "provider": provider,
    "pane_pid": pane_pid,
    "pane_birth": pane_birth,
    "socket": socket,
}
if provider == "opencode":
    handle = connection.execute(
        "SELECT endpoint_port, observer_pid, observer_birth, observer_status "
        "FROM opencode_runtime_handles WHERE runtime_id = ?", (runtime_id,)
    ).fetchone()
    if handle is None or handle[1] is None or handle[2] is None or handle[3] != "ready":
        raise SystemExit("OpenCode observer identity is not ready")
    observer_pid = int(handle[1])
    observer_birth = process_birth(observer_pid)
    if observer_birth != handle[2]:
        raise SystemExit("observer process birth does not match persisted handle")
    identity.update({
        "endpoint_port": int(handle[0]),
        "observer_pid": observer_pid,
        "observer_birth": observer_birth,
    })
json.dump(identity, sys.stdout, sort_keys=True)
PY
}

codex_runtime_id="$(runtime_id_for_workstream "$codex_workstream_id")"
opencode_runtime_id="$(runtime_id_for_workstream "$opencode_workstream_id")"
codex_identity_file="$task_root/codex-runtime-identity.json"
opencode_identity_file="$task_root/opencode-runtime-identity.json"
for _ in {1..100}; do
    if record_runtime_identity "$codex_runtime_id" "$codex_identity_file" \
        && record_runtime_identity "$opencode_runtime_id" "$opencode_identity_file"; then
        break
    fi
    sleep 0.1
done
[[ -s "$codex_identity_file" && -s "$opencode_identity_file" ]]

# Both attachment paths use the native pane directly. Detaching the terminal
# leaves the provider Runtime and observer alive for independent switching.
attach_once() {
    local workstream_id="$1" marker="$2" name="$3" entrypoint="$4"
    local runtime_id socket session driver_socket driver_session done_file command client
    runtime_id="$(runtime_id_for_workstream "$workstream_id")"
    socket="$state_root/run/runtime-$runtime_id/tmux.sock"
    session="$(env -u TMUX tmux -S "$socket" list-sessions -F '#{session_name}' | head -n 1)"
    driver_socket="$task_root/$name.sock"; driver_session="wsnav-$name"; done_file="$task_root/$name.done"
    if [[ "$entrypoint" == "local" ]]; then
        printf -v command '%q --state-root %q attach %q && touch %q' "$wsnav_bin" "$state_root" "$workstream_id" "$done_file"
    else
        # The hidden `_attach` command is the RemoteAttach native-terminal
        # service path used by SSH after its control-plane preflight.
        printf -v command '%q --state-root %q _attach %q && touch %q' "$wsnav_bin" "$state_root" "$runtime_id" "$done_file"
    fi
    env -u TMUX tmux -u -f /dev/null -S "$driver_socket" new-session -d -s "$driver_session" -c "$repository" "$command"
    client=""
    for _ in {1..100}; do
        client="$(env -u TMUX tmux -S "$socket" list-clients -F '#{client_tty}' 2>/dev/null | head -n 1 || true)"
        if [[ -n "$client" ]] && env -u TMUX tmux -S "$socket" capture-pane -p -J -t "$session:0.0" 2>/dev/null | grep -F "$marker" >/dev/null \
            && env -u TMUX tmux -S "$driver_socket" capture-pane -p -J -t "$driver_session:0.0" 2>/dev/null | grep -F "$marker" >/dev/null; then break; fi
        sleep 0.1
    done
    [[ -n "$client" ]]
    env -u TMUX tmux -S "$socket" detach-client -t "$client"
    for _ in {1..100}; do [[ -f "$done_file" ]] && break; sleep 0.1; done
    [[ -f "$done_file" ]]
    env -u TMUX tmux -S "$driver_socket" kill-server >/dev/null 2>&1 || true
    grep -F 'private runtime: live' <<<"$($wsnav_bin --state-root "$state_root" status "$workstream_id")" >/dev/null
}

attach_once "$codex_workstream_id" MIXED_CODEX_NATIVE_SURFACE codex local
attach_once "$opencode_workstream_id" MIXED_OPENCODE_NATIVE_SURFACE opencode remote

if grep -E -- '--pure|--model|--agent|--prompt' "$provider_args" >/dev/null; then
    echo "provider launch received a forbidden WSNav-owned flag" >&2
    exit 1
fi
if grep -R -E 'MIXED_(CODEX|OPENCODE)' "$state_root" >/dev/null 2>&1; then
    echo "native provider payload leaked into durable WSNav state" >&2
    exit 1
fi

# Re-record the exact identities immediately before park. This proves the
# attachment paths did not silently rotate or adopt a different process.
record_runtime_identity "$codex_runtime_id" "$codex_identity_file"
record_runtime_identity "$opencode_runtime_id" "$opencode_identity_file"

"$wsnav_bin" --state-root "$state_root" park "$opencode_workstream_id"
"$wsnav_bin" --state-root "$state_root" park "$codex_workstream_id"
python3 - "$state_root/host.sqlite" "$state_root/run" "$codex_identity_file" "$opencode_identity_file" <<'PY'
import json, socket, sqlite3, sys
from pathlib import Path

db, run_root, *identity_paths = sys.argv[1:]

def process_birth(pid):
    stat = Path(f"/proc/{pid}/stat").read_text()
    close_paren = stat.rfind(")")
    return stat[close_paren + 2:].split()[19]

for identity_path in identity_paths:
    identity = json.loads(Path(identity_path).read_text())
    for key in ("pane", "observer"):
        pid = identity.get(f"{key}_pid")
        birth = identity.get(f"{key}_birth")
        if pid is None:
            continue
        try:
            current_birth = process_birth(pid)
        except (FileNotFoundError, IndexError):
            continue
        if current_birth == birth:
            raise SystemExit(f"exact {key} process survived park: {pid}")
    socket_path = Path(identity["socket"])
    if socket_path.exists():
        raise SystemExit(f"private Runtime socket survived park: {socket_path}")
    if identity["provider"] == "opencode":
        port = identity["endpoint_port"]
        probe = socket.socket()
        probe.settimeout(0.25)
        try:
            if probe.connect_ex(("127.0.0.1", port)) == 0:
                raise SystemExit(f"OpenCode endpoint port remained open: {port}")
        finally:
            probe.close()

private_sockets = list(Path(run_root).rglob("tmux.sock")) if Path(run_root).exists() else []
if private_sockets:
    raise SystemExit(f"private Runtime sockets survived park: {private_sockets}")
connection = sqlite3.connect(db)
if connection.execute("SELECT COUNT(*) FROM opencode_runtime_handles").fetchone()[0] != 0:
    raise SystemExit("OpenCode handle survived park")
if connection.execute("SELECT COUNT(*) FROM runtimes WHERE lifecycle != 'stopped'").fetchone()[0] != 0:
    raise SystemExit("a mixed-provider Runtime survived park")
PY

ordinary_tmux_after="$(env -u TMUX tmux list-sessions -F '#{session_name}:#{session_created}:#{session_windows}' -O name 2>/dev/null || true)"
[[ "$ordinary_tmux_before" == "$ordinary_tmux_after" ]]
codex_workstream_id=""
opencode_workstream_id=""
cleanup
trap - EXIT
[[ ! -e "$task_root" && ! -e "$state_root" && ! -e "$codex_home" && ! -e "$opencode_db" ]]
printf 'D8.1 disposable mixed-provider acceptance passed\n'
