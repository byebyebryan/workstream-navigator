#!/usr/bin/env python3
"""Probe the OpenCode native-TUI runtime, observer, and exact HTTP fork boundary.

The probe is deliberately operator-gated because it starts native OpenCode TUIs
and sends harmless prompts through disposable provider state. It records only
bounded assertions and identifier digests; all provider state, tmux servers,
and temporary XDG roots are removed before the result is written.
"""

from __future__ import annotations

import argparse
import hashlib
import http.client
import json
import os
import socket
import subprocess
import sys
import tempfile
import threading
import time
import urllib.error
import urllib.request
from pathlib import Path
from typing import Any

from opencode_support import (
    environment_for_directory,
    isolated_environment,
    remove_root,
)

MODEL = "opencode-go/deepseek-v4-flash"
BASELINE_MARKER = "WSNAV_OC_RUNTIME_BASELINE"
EVENT_MARKER = "WSNAV_OC_RUNTIME_EVENT"
ACTIVE_MARKER = "WSNAV_OC_RUNTIME_ACTIVE"


class StudyFailure(RuntimeError):
    pass


class StudyBlocked(RuntimeError):
    pass


def free_port() -> int:
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return int(sock.getsockname()[1])


def digest(value: str) -> str:
    return hashlib.sha256(value.encode()).hexdigest()[:16]


def private_tmux(
    socket_path: Path,
    *args: str,
    check: bool = True,
    env: dict[str, str] | None = None,
) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        ["tmux", "-S", str(socket_path), *args],
        capture_output=True,
        text=True,
        check=check,
        env=env,
    )


def pane_pid(socket_path: Path, session: str) -> int:
    value = private_tmux(
        socket_path,
        "display-message",
        "-p",
        "-t",
        f"{session}:0.0",
        "#{pane_pid}",
    ).stdout.strip()
    if not value.isdigit():
        raise StudyFailure("private tmux did not return a provider pane PID")
    return int(value)


def process_tree(root_pid: int) -> set[int]:
    children: dict[int, set[int]] = {}
    proc_root = Path("/proc")
    for entry in proc_root.iterdir():
        if not entry.name.isdigit():
            continue
        try:
            fields = (entry / "stat").read_text(encoding="utf-8").split()
            parent = int(fields[3])
            children.setdefault(parent, set()).add(int(entry.name))
        except (OSError, ValueError, IndexError):
            continue
    seen = {root_pid}
    pending = [root_pid]
    while pending:
        current = pending.pop()
        for child in children.get(current, set()):
            if child not in seen:
                seen.add(child)
                pending.append(child)
    return seen


def tree_has_opencode(root_pid: int) -> bool:
    for pid in process_tree(root_pid):
        try:
            command = (
                (Path("/proc") / str(pid) / "cmdline").read_bytes().replace(b"\0", b" ")
            )
        except OSError:
            continue
        if b"opencode" in command:
            return True
    return False


def request(
    base: str,
    path: str,
    method: str = "GET",
    body: dict[str, Any] | None = None,
    timeout: int = 15,
) -> Any:
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(f"{base}{path}", data=data, method=method)
    req.add_header("Content-Type", "application/json")
    with urllib.request.urlopen(req, timeout=timeout) as response:
        return json.loads(response.read().decode())


def wait_health(base: str, deadline: float) -> dict[str, Any]:
    while time.time() < deadline:
        try:
            result = request(base, "/global/health", timeout=3)
            if isinstance(result, dict) and result.get("healthy"):
                return result
        except (
            http.client.HTTPException,
            urllib.error.URLError,
            urllib.error.HTTPError,
            json.JSONDecodeError,
            TimeoutError,
        ):
            time.sleep(0.25)
    raise StudyBlocked("native OpenCode TUI server did not become ready")


def bounded_event(
    payload: dict[str, Any], observed_session_id: str
) -> dict[str, Any] | None:
    properties = payload.get("properties", {})
    if not isinstance(properties, dict):
        return None
    info = properties.get("info")
    if not isinstance(info, dict):
        info = {}
    session_id = properties.get("sessionID") or info.get("sessionID")
    if session_id != observed_session_id:
        return None
    status = properties.get("status")
    if not isinstance(status, dict):
        status = {}
    return {
        "type": payload.get("type"),
        "session_id": session_id,
        "message_id": info.get("id"),
        "role": info.get("role"),
        "finish": info.get("finish"),
        "completed": isinstance(info.get("time"), dict)
        and info["time"].get("completed") is not None,
        "status": status.get("type"),
    }


def run_cli(
    args: list[str], cwd: Path, env: dict[str, str], timeout: int = 180
) -> subprocess.CompletedProcess[str]:
    proc = subprocess.run(
        ["opencode", "--pure", *args],
        capture_output=True,
        text=True,
        timeout=timeout,
        cwd=cwd,
        env=environment_for_directory(env, cwd),
        check=False,
    )
    if proc.returncode != 0:
        raise StudyBlocked(f"opencode command failed: {proc.stderr[-400:]}")
    return proc


def session_id_from_run(output: str) -> str:
    for line in output.splitlines():
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue
        if event.get("type") == "step_start" and event.get("sessionID"):
            return str(event["sessionID"])
    raise StudyFailure("opencode run emitted no session ID")


def create_session(project: Path, env: dict[str, str], marker: str) -> str:
    prompt = (
        f"Reply with the exact token {marker} and nothing else. "
        "Do not use tools, inspect files, or make changes."
    )
    return session_id_from_run(
        run_cli(
            ["run", "--model", MODEL, "--format", "json", prompt],
            project,
            env,
        ).stdout
    )


def start_tui(
    root: Path,
    project: Path,
    env: dict[str, str],
    session_id: str,
) -> tuple[Path, str, int, str]:
    socket_path = root / f"tmux-{digest(session_id)}.sock"
    session = f"wsnav-{digest(session_id)}"
    port = free_port()
    command = [
        "opencode",
        str(project),
        "--pure",
        "--hostname",
        "127.0.0.1",
        "--port",
        str(port),
        "--session",
        session_id,
    ]
    try:
        private_tmux(
            socket_path,
            "-f",
            "/dev/null",
            "new-session",
            "-d",
            "-s",
            session,
            "-x",
            "120",
            "-y",
            "42",
            "--",
            *command,
            env=environment_for_directory(env, project),
        )
        pid = pane_pid(socket_path, session)
        base = f"http://127.0.0.1:{port}"
        health = wait_health(base, time.time() + 30)
        if not tree_has_opencode(pid):
            raise StudyFailure("private pane PID is not correlated with OpenCode")
        if str(health.get("version", "")) != "1.18.11":
            raise StudyFailure("OpenCode version changed during the pinned study")
        return socket_path, session, pid, base
    except Exception:
        private_tmux(socket_path, "kill-server", check=False)
        raise


def stream_events(
    base: str,
    observed_session_id: str,
    stop: threading.Event,
    events: list[dict[str, Any]],
) -> threading.Thread:
    def worker() -> None:
        try:
            with urllib.request.urlopen(f"{base}/global/event", timeout=45) as response:
                while not stop.is_set():
                    line = response.readline()
                    if not line:
                        return
                    if not line.startswith(b"data:"):
                        continue
                    try:
                        envelope = json.loads(line[5:].strip())
                        payload = envelope.get("payload", {})
                    except (json.JSONDecodeError, AttributeError):
                        continue
                    if not isinstance(payload, dict):
                        continue
                    event = bounded_event(payload, observed_session_id)
                    if event is None:
                        continue
                    events.append(event)
        except (OSError, urllib.error.URLError):
            return

    thread = threading.Thread(target=worker, daemon=True)
    thread.start()
    return thread


def event_has(events: list[dict[str, Any]], **expected: Any) -> bool:
    return any(
        all(event.get(key) == value for key, value in expected.items())
        for event in events
    )


def messages(base: str, session_id: str) -> list[dict[str, Any]]:
    result = request(base, f"/session/{session_id}/message")
    return result if isinstance(result, list) else []


def message_text(message: dict[str, Any]) -> str:
    parts = message.get("parts", [])
    return "".join(
        str(part.get("text", ""))
        for part in parts
        if isinstance(part, dict) and part.get("type") == "text"
    )


def last_completed_assistant(base: str, session_id: str) -> str | None:
    candidates: list[tuple[int, str]] = []
    for message in messages(base, session_id):
        info = message.get("info", {})
        if not isinstance(info, dict) or info.get("role") != "assistant":
            continue
        time_info = info.get("time", {})
        completed = time_info.get("completed") if isinstance(time_info, dict) else None
        if completed is not None and info.get("id"):
            candidates.append((int(completed), str(info["id"])))
    return max(candidates)[1] if candidates else None


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--confirm-live-opencode", action="store_true")
    parser.add_argument("--result", type=Path)
    args = parser.parse_args()

    assertions = {
        "operator_confirmed": args.confirm_live_opencode,
        "native_tui_server_ready": False,
        "provider_pid_correlated": False,
        "exact_resume_session_visible": False,
        "lifecycle_events_filtered": False,
        "child_session_events_ignored": False,
        "second_runtime_has_distinct_endpoint": False,
        "runtime_events_do_not_cross": False,
        "http_fork_returns_distinct_session": False,
        "http_fork_preserves_settled_prefix": False,
        "http_fork_omits_in_flight_turn": False,
        "http_fork_lineage_absent": False,
        "cleanup_complete": False,
    }
    root: Path | None = None
    runtimes: list[tuple[Path, str]] = []
    status = "blocked"
    reason = "operator-confirmation-required"
    source_id: str | None = None
    destination_id: str | None = None
    active: subprocess.Popen[str] | None = None
    try:
        if not args.confirm_live_opencode:
            raise StudyBlocked(reason)
        root = Path(tempfile.mkdtemp(prefix="wsnav-opencode-runtime."))
        env = isolated_environment(root)
        project = root / "project"
        project.mkdir()
        source_id = create_session(project, env, BASELINE_MARKER)
        second_id = create_session(project, env, EVENT_MARKER)

        first = start_tui(root, project, env, source_id)
        runtimes.append((first[0], first[1]))
        second = start_tui(root, project, env, second_id)
        runtimes.append((second[0], second[1]))
        assertions["native_tui_server_ready"] = True
        assertions["provider_pid_correlated"] = first[2] != second[2]
        assertions["exact_resume_session_visible"] = bool(
            request(first[3], f"/session/{source_id}").get("id") == source_id
        )
        assertions["second_runtime_has_distinct_endpoint"] = first[3] != second[3]

        first_events: list[dict[str, Any]] = []
        second_events: list[dict[str, Any]] = []
        first_stop = threading.Event()
        second_stop = threading.Event()
        first_thread = stream_events(first[3], source_id, first_stop, first_events)
        second_thread = stream_events(second[3], second_id, second_stop, second_events)
        run_cli(
            [
                "run",
                "--attach",
                first[3],
                "--session",
                source_id,
                "--model",
                MODEL,
                "--format",
                "json",
                EVENT_MARKER,
            ],
            project,
            env,
        )
        deadline = time.time() + 30
        while time.time() < deadline and not event_has(
            first_events, type="session.idle", session_id=source_id
        ):
            time.sleep(0.25)
        first_stop.set()
        second_stop.set()
        first_thread.join(timeout=2)
        second_thread.join(timeout=2)
        assertions["lifecycle_events_filtered"] = (
            event_has(first_events, type="session.status", session_id=source_id)
            and event_has(first_events, type="session.idle", session_id=source_id)
            and event_has(first_events, type="message.updated", session_id=source_id)
        )
        assertions["runtime_events_do_not_cross"] = all(
            event.get("session_id") != source_id for event in second_events
        )
        child_fixture = {
            "type": "session.status",
            "properties": {
                "sessionID": digest("provider-child"),
                "status": {"type": "idle"},
            },
        }
        assertions["child_session_events_ignored"] = (
            bounded_event(child_fixture, source_id) is None
        )

        settled_id = last_completed_assistant(first[3], source_id)
        if settled_id is None:
            raise StudyFailure("no settled assistant message ID was observed")
        active = subprocess.Popen(
            [
                "opencode",
                "--pure",
                "run",
                "--attach",
                first[3],
                "--session",
                source_id,
                "--model",
                MODEL,
                "--format",
                "json",
                "--auto",
                (
                    "Run the shell command sleep 25 exactly once, then reply with "
                    f"{ACTIVE_MARKER}."
                ),
            ],
            cwd=project,
            env=environment_for_directory(env, project),
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
        deadline = time.time() + 30
        while time.time() < deadline:
            status_map = request(first[3], "/session/status")
            source_status = (
                status_map.get(source_id, {}) if isinstance(status_map, dict) else {}
            )
            if isinstance(source_status, dict) and source_status.get("type") == "busy":
                break
            time.sleep(0.25)
        else:
            raise StudyFailure("source never entered busy state before HTTP fork")
        forked = request(
            first[3],
            f"/session/{source_id}/fork",
            method="POST",
            body={"messageID": settled_id},
        )
        destination_id = forked.get("id") if isinstance(forked, dict) else None
        assertions["http_fork_returns_distinct_session"] = bool(
            destination_id and destination_id != source_id
        )
        if destination_id is None:
            raise StudyFailure("HTTP fork returned no destination ID")
        active.kill()
        active.wait(timeout=10)
        destination_messages = messages(first[3], destination_id)
        destination_text = "\n".join(
            message_text(message) for message in destination_messages
        )
        assertions["http_fork_preserves_settled_prefix"] = (
            BASELINE_MARKER in destination_text and EVENT_MARKER in destination_text
        )
        assertions["http_fork_omits_in_flight_turn"] = (
            ACTIVE_MARKER not in destination_text
        )
        children = request(first[3], f"/session/{source_id}/children")
        child_ids = {child.get("id") for child in children if isinstance(child, dict)}
        assertions["http_fork_lineage_absent"] = destination_id not in child_ids
        required = (
            "operator_confirmed",
            "native_tui_server_ready",
            "provider_pid_correlated",
            "exact_resume_session_visible",
            "lifecycle_events_filtered",
            "child_session_events_ignored",
            "second_runtime_has_distinct_endpoint",
            "runtime_events_do_not_cross",
            "http_fork_returns_distinct_session",
            "http_fork_preserves_settled_prefix",
            "http_fork_omits_in_flight_turn",
            "http_fork_lineage_absent",
        )
        if not all(assertions[name] for name in required):
            raise StudyFailure("OpenCode runtime contract assertions incomplete")
        status, reason = "pass", "native-runtime-observer-and-http-fork-confirmed"
    except StudyBlocked as error:
        status, reason = "blocked", str(error)
    except StudyFailure as error:
        status, reason = "falsified", str(error)
    except (
        OSError,
        TimeoutError,
        subprocess.TimeoutExpired,
        urllib.error.URLError,
    ) as error:
        status, reason = "blocked", f"harness-error:{type(error).__name__}"
    finally:
        if active is not None and active.poll() is None:
            active.kill()
            active.wait(timeout=10)
        for socket_path, _session in runtimes:
            private_tmux(socket_path, "kill-server", check=False)
        assertions["cleanup_complete"] = remove_root(root)

    result = {
        "study": "opencode-runtime-contract",
        "status": status,
        "reason": reason,
        "opencode_version": "1.18.11",
        "assertions": assertions,
        "source_session_digest": digest(source_id) if source_id else None,
        "destination_session_digest": digest(destination_id)
        if destination_id
        else None,
    }
    if args.result:
        args.result.write_text(json.dumps(result, indent=2))
        os.chmod(args.result, 0o600)
    else:
        print(json.dumps(result, indent=2))
    return 0 if status == "pass" else 1


if __name__ == "__main__":
    sys.exit(main())
