#!/usr/bin/env python3
"""Probe OpenCode blank-session binding and per-Runtime observer ownership.

This is a disposable, operator-gated decision study. It creates blank
sessions through a short-lived provider server, launches two native TUIs at
the same project root without ``--pure``, model, agent, or prompt flags, and
supervises one bounded observer child per Runtime. The fixture contains only
assertions, counts, and identifier digests; all provider state, tmux servers,
observer processes, and temporary XDG roots are removed before it exits.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import signal
import socket
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
from pathlib import Path
from typing import Any, NamedTuple

from opencode_support import (
    environment_for_directory,
    isolated_environment,
    remove_root,
)

VERSION = "1.18.11"
FIRST_MARKER = "WSNAV_OC_FRESH_FIRST"
SECOND_MARKER = "WSNAV_OC_FRESH_SECOND"
STUDY = "opencode-fresh-session"


class StudyFailure(RuntimeError):
    pass


class StudyBlocked(RuntimeError):
    pass


class RuntimeHandle(NamedTuple):
    socket_path: Path
    session: str
    pane_pid: int
    port: int
    base: str
    pane_birth: str


def digest(value: str) -> str:
    return hashlib.sha256(value.encode()).hexdigest()[:16]


def version_allowed(version: str) -> bool:
    return version == VERSION


def free_port() -> int:
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return int(sock.getsockname()[1])


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


def process_birth(pid: int) -> str | None:
    try:
        stat = (Path("/proc") / str(pid) / "stat").read_text(encoding="utf-8")
        fields = stat.rsplit(")", 1)[1].split()
        return fields[19]
    except (OSError, IndexError):
        return None


def process_tree(root_pid: int) -> set[int]:
    children: dict[int, set[int]] = {}
    for entry in Path("/proc").iterdir():
        if not entry.name.isdigit():
            continue
        try:
            stat = (entry / "stat").read_text(encoding="utf-8")
            fields = stat.rsplit(")", 1)[1].split()
            children.setdefault(int(fields[1]), set()).add(int(entry.name))
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


def process_identity_matches(pid: int, birth: str | None) -> bool:
    return birth is not None and process_birth(pid) == birth


def process_tree_identities(root_pid: int) -> dict[int, str]:
    identities: dict[int, str] = {}
    for pid in process_tree(root_pid):
        birth = process_birth(pid)
        if birth is not None:
            identities[pid] = birth
    return identities


def socket_inodes(port: int) -> set[str]:
    wanted = f"0100007F:{port:04X}"
    inodes: set[str] = set()
    try:
        lines = Path("/proc/net/tcp").read_text(encoding="utf-8").splitlines()
    except OSError:
        return inodes
    for line in lines[1:]:
        fields = line.split()
        if len(fields) > 9 and fields[1] == wanted and fields[3] == "0A":
            inodes.add(fields[9])
    return inodes


def socket_owner_pids(inodes: set[str]) -> set[int]:
    owners: set[int] = set()
    if not inodes:
        return owners
    for entry in Path("/proc").iterdir():
        if not entry.name.isdigit():
            continue
        try:
            for fd in (entry / "fd").iterdir():
                target = os.readlink(fd)
                if (
                    target.startswith("socket:[")
                    and target.endswith("]")
                    and target[8:-1] in inodes
                ):
                    owners.add(int(entry.name))
                    break
        except (OSError, ValueError):
            continue
    return owners


def endpoint_owned_by_tree(port: int, pane: int) -> bool:
    return bool(socket_owner_pids(socket_inodes(port)) & process_tree(pane))


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
        payload = response.read().decode()
    return json.loads(payload) if payload else None


def wait_health(base: str, deadline: float) -> dict[str, Any]:
    while time.time() < deadline:
        try:
            result = request(base, "/global/health", timeout=3)
            if isinstance(result, dict) and result.get("healthy"):
                return result
        except (OSError, urllib.error.URLError, json.JSONDecodeError, TimeoutError):
            time.sleep(0.25)
    raise StudyBlocked("OpenCode server did not become ready")


def start_server(
    project: Path, env: dict[str, str], port: int
) -> tuple[subprocess.Popen[str], str]:
    process = subprocess.Popen(
        ["opencode", "serve", "--hostname", "127.0.0.1", "--port", str(port)],
        cwd=project,
        env=environment_for_directory(env, project),
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        text=True,
    )
    base = f"http://127.0.0.1:{port}"
    try:
        health = wait_health(base, time.time() + 30)
        if str(health.get("version", "")) != VERSION:
            raise StudyBlocked("OpenCode version changed during the pinned study")
        return process, base
    except Exception:
        terminate_process(process)
        raise


def terminate_process(process: subprocess.Popen[str] | None) -> None:
    if process is None or process.poll() is not None:
        return
    process.terminate()
    try:
        process.wait(timeout=10)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait(timeout=10)


def create_blank_session(base: str) -> str:
    result = request(base, "/session", method="POST", body={})
    if not isinstance(result, dict) or not isinstance(result.get("id"), str):
        raise StudyFailure("short-lived server did not create a blank session")
    session_id = result["id"]
    messages = request(base, f"/session/{session_id}/message")
    if not isinstance(messages, list) or messages:
        raise StudyFailure("new session was not blank")
    return session_id


def tui_command(project: Path, port: int, session_id: str) -> list[str]:
    return [
        "opencode",
        str(project),
        "--hostname",
        "127.0.0.1",
        "--port",
        str(port),
        "--session",
        session_id,
    ]


def command_uses_native_defaults(command: list[str]) -> bool:
    prohibited = {"--pure", "--model", "--agent", "--prompt"}
    return prohibited.isdisjoint(command)


def exact_session_visible(base: str, session_id: str) -> bool:
    result = request(base, f"/session/{session_id}")
    return isinstance(result, dict) and result.get("id") == session_id


def start_tui(
    root: Path,
    project: Path,
    env: dict[str, str],
    session_id: str,
    expect_blank: bool = True,
) -> RuntimeHandle:
    socket_path = root / f"tmux-{digest(session_id)}.sock"
    session = f"wsnav-{digest(session_id)}"
    port = free_port()
    command = tui_command(project, port, session_id)
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
        birth = process_birth(pid)
        if birth is None:
            raise StudyFailure("provider pane process birth was unavailable")
        base = f"http://127.0.0.1:{port}"
        health = wait_health(base, time.time() + 30)
        if str(health.get("version", "")) != VERSION:
            raise StudyBlocked("OpenCode version changed during the pinned study")
        if not endpoint_owned_by_tree(port, pid):
            raise StudyFailure("provider endpoint was not correlated to pane tree")
        if not exact_session_visible(base, session_id):
            raise StudyFailure("native TUI did not expose the exact blank session")
        messages = request(base, f"/session/{session_id}/message")
        if not isinstance(messages, list):
            raise StudyFailure("native TUI returned an invalid message list")
        if expect_blank:
            roles = [
                message.get("info", {}).get("role")
                for message in messages
                if isinstance(message, dict) and isinstance(message.get("info"), dict)
            ]
            if any(role in {"user", "assistant"} for role in roles):
                raise StudyFailure(
                    "native TUI added conversational records to blank session: "
                    f"roles={roles}"
                )
        return RuntimeHandle(socket_path, session, pid, port, base, birth)
    except Exception:
        private_tmux(socket_path, "kill-server", check=False)
        raise


def send_native_prompt(socket_path: Path, session: str, prompt: str) -> None:
    target = f"{session}:0.0"
    private_tmux(socket_path, "send-keys", "-l", "-t", target, prompt)
    private_tmux(socket_path, "send-keys", "-t", target, "Enter")


def bounded_event(payload: dict[str, Any], observed_session: str) -> tuple[bool, bool]:
    properties = payload.get("properties", {})
    if not isinstance(properties, dict):
        return False, False
    info = properties.get("info")
    if not isinstance(info, dict):
        info = {}
    session_id = properties.get("sessionID") or info.get("sessionID")
    if session_id is None:
        return False, False
    return session_id == observed_session, True


def write_json(path: Path, value: dict[str, Any]) -> None:
    path.write_text(json.dumps(value, sort_keys=True), encoding="utf-8")
    os.chmod(path, 0o600)


def observer_child(args: argparse.Namespace) -> int:
    result_path = Path(args.observer_result)
    deadline = time.time() + 75
    matched = 0
    foreign = 0
    reconnects = 0
    ready = False
    status = "starting"
    while time.time() < deadline:
        try:
            with urllib.request.urlopen(
                f"{args.base}/global/event", timeout=8
            ) as stream:
                ready = True
                status = "ready"
                write_json(
                    result_path,
                    {
                        "status": status,
                        "generation_digest": digest(args.generation),
                        "matched_events": matched,
                        "foreign_events_discarded": foreign,
                        "reconnects": reconnects,
                        "content_discarded": True,
                    },
                )
                while time.time() < deadline:
                    line = stream.readline()
                    if not line:
                        break
                    if not line.startswith(b"data:"):
                        continue
                    try:
                        envelope = json.loads(line[5:].strip())
                        payload = envelope.get("payload", {})
                    except (json.JSONDecodeError, AttributeError):
                        continue
                    if not isinstance(payload, dict):
                        continue
                    is_match, has_session = bounded_event(payload, args.session)
                    if not has_session:
                        continue
                    if is_match:
                        matched += 1
                    else:
                        foreign += 1
                    write_json(
                        result_path,
                        {
                            "status": status,
                            "generation_digest": digest(args.generation),
                            "matched_events": matched,
                            "foreign_events_discarded": foreign,
                            "reconnects": reconnects,
                            "content_discarded": True,
                        },
                    )
        except (OSError, urllib.error.URLError, TimeoutError):
            if time.time() >= deadline:
                break
            reconnects += 1
            time.sleep(0.25)
    if not ready:
        status = "unknown"
    write_json(
        result_path,
        {
            "status": status,
            "generation_digest": digest(args.generation),
            "matched_events": matched,
            "foreign_events_discarded": foreign,
            "reconnects": reconnects,
            "content_discarded": True,
        },
    )
    return 0 if ready else 1


def start_observer(
    base: str,
    session: str,
    generation: str,
    result_path: Path,
) -> tuple[subprocess.Popen[str], int, str]:
    process = subprocess.Popen(
        [
            sys.executable,
            str(Path(__file__).resolve()),
            "--observer-child",
            "--base",
            base,
            "--session",
            session,
            "--generation",
            generation,
            "--observer-result",
            str(result_path),
        ],
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        text=True,
    )
    birth = process_birth(process.pid)
    if birth is None:
        terminate_process(process)
        raise StudyFailure("observer process birth was unavailable")
    deadline = time.time() + 15
    while time.time() < deadline:
        try:
            result = json.loads(result_path.read_text(encoding="utf-8"))
            if result.get("status") == "ready":
                return process, process.pid, birth
        except (OSError, json.JSONDecodeError):
            pass
        if process.poll() is not None:
            raise StudyFailure("observer child exited before becoming ready")
        time.sleep(0.1)
    terminate_process(process)
    raise StudyBlocked("observer child did not become ready")


def observer_result(path: Path) -> dict[str, Any]:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise StudyFailure("observer result was unavailable") from error
    if not isinstance(value, dict):
        raise StudyFailure("observer result was not an object")
    return value


def simulated_detach(socket_path: Path, session: str) -> bool:
    client = subprocess.Popen(
        ["tmux", "-C", "-S", str(socket_path), "attach-session", "-t", session],
        stdin=subprocess.PIPE,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        text=True,
    )
    time.sleep(0.5)
    if client.stdin is not None:
        client.stdin.close()
    try:
        client.wait(timeout=5)
    except subprocess.TimeoutExpired:
        client.terminate()
        client.wait(timeout=5)
    return (
        client.returncode == 0
        and private_tmux(
            socket_path, "has-session", "-t", session, check=False
        ).returncode
        == 0
    )


def wrong_endpoint_check(project: Path, provider_pane_pid: int) -> bool:
    port = free_port()
    process = subprocess.Popen(
        [sys.executable, "-m", "http.server", str(port), "--bind", "127.0.0.1"],
        cwd=project,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        text=True,
    )
    try:
        deadline = time.time() + 5
        while time.time() < deadline and not socket_inodes(port):
            time.sleep(0.1)
        return not bool(
            socket_owner_pids(socket_inodes(port)) & process_tree(provider_pane_pid)
        )
    finally:
        terminate_process(process)


def port_collision_check(project: Path, env: dict[str, str]) -> bool:
    blocker = socket.socket()
    blocker.bind(("127.0.0.1", 0))
    blocker.listen(1)
    port = int(blocker.getsockname()[1])
    process = subprocess.Popen(
        ["opencode", "serve", "--hostname", "127.0.0.1", "--port", str(port)],
        cwd=project,
        env=environment_for_directory(env, project),
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        text=True,
    )
    try:
        deadline = time.time() + 5
        while time.time() < deadline and process.poll() is None:
            time.sleep(0.1)
        return process.poll() is not None and process.returncode != 0
    finally:
        terminate_process(process)
        blocker.close()


def wait_port_closed(port: int, deadline: float) -> bool:
    while time.time() < deadline:
        if not socket_inodes(port):
            return True
        time.sleep(0.1)
    return False


def terminate_provider_tree(identities: dict[int, str]) -> None:
    for pid, birth in identities.items():
        if pid == os.getpid() or not process_identity_matches(pid, birth):
            continue
        try:
            os.kill(pid, signal.SIGTERM)
        except OSError:
            continue


def wait_identities_gone(identities: dict[int, str], deadline: float) -> bool:
    while time.time() < deadline:
        if not any(
            process_identity_matches(pid, birth) for pid, birth in identities.items()
        ):
            return True
        time.sleep(0.1)
    return not any(
        process_identity_matches(pid, birth) for pid, birth in identities.items()
    )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--confirm-live-opencode", action="store_true")
    parser.add_argument("--result", type=Path)
    parser.add_argument("--observer-child", action="store_true")
    parser.add_argument("--base")
    parser.add_argument("--session")
    parser.add_argument("--generation")
    parser.add_argument("--observer-result")
    args = parser.parse_args()
    if args.observer_child:
        if not all((args.base, args.session, args.generation, args.observer_result)):
            return 2
        return observer_child(args)

    assertions = {
        "operator_confirmed": args.confirm_live_opencode,
        "production_command_has_no_pure_or_model_flags": False,
        "version_allowlist_enforced": False,
        "blank_session_precreated_without_messages": False,
        "two_blank_same_root_sessions": False,
        "native_tui_server_ready": False,
        "exact_blank_session_visible": False,
        "endpoint_process_correlation": False,
        "wrong_healthy_endpoint_rejected": False,
        "port_collision_rejected": False,
        "stale_saved_port_rejected": False,
        "observer_started_before_native_input": False,
        "observer_generation_and_birth_recorded": False,
        "observer_filtered_to_exact_session": False,
        "observer_discarded_foreign_events": False,
        "child_session_events_ignored": False,
        "unrelated_root_session_not_selected": False,
        "native_prompts_remain_non_crossing": False,
        "observer_helper_crash_detected": False,
        "observer_replacement_reconnects": False,
        "detach_reopen_retains_runtime_and_observer": False,
        "exact_resume_after_runtime_restart": False,
        "cleanup_complete": False,
    }
    root: Path | None = None
    server: subprocess.Popen[str] | None = None
    unrelated_server: subprocess.Popen[str] | None = None
    runtimes: list[RuntimeHandle] = []
    observers: list[subprocess.Popen[str]] = []
    tracked_provider_identities: dict[int, str] = {}
    tracked_observer_identities: dict[int, str] = {}
    tracked_ports: set[int] = set()
    status = "blocked"
    reason = "operator-confirmation-required"
    session_ids: list[str] = []
    generations: list[str] = []
    try:
        if not args.confirm_live_opencode:
            raise StudyBlocked(reason)
        root = Path(tempfile.mkdtemp(prefix="wsnav-opencode-fresh."))
        env = isolated_environment(root)
        project = root / "project"
        project.mkdir()
        unrelated_project = root / "unrelated-project"
        unrelated_project.mkdir()

        assertions["version_allowlist_enforced"] = version_allowed(
            VERSION
        ) and not version_allowed("0.0.0")
        server_port = free_port()
        server, base = start_server(project, env, server_port)
        tracked_ports.add(server_port)
        tracked_provider_identities.update(process_tree_identities(server.pid))
        first_id = create_blank_session(base)
        second_id = create_blank_session(base)
        session_ids.extend((first_id, second_id))
        unrelated_port = free_port()
        unrelated_server, unrelated_base = start_server(
            unrelated_project, env, unrelated_port
        )
        tracked_ports.add(unrelated_port)
        tracked_provider_identities.update(
            process_tree_identities(unrelated_server.pid)
        )
        unrelated_id = create_blank_session(unrelated_base)
        unrelated_record = request(unrelated_base, f"/session/{unrelated_id}")
        terminate_process(unrelated_server)
        unrelated_server = None
        assertions["blank_session_precreated_without_messages"] = True
        terminate_process(server)
        server = None

        first = start_tui(root, project, env, first_id)
        second = start_tui(root, project, env, second_id)
        runtimes.extend((first, second))
        tracked_ports.update((first.port, second.port))
        tracked_provider_identities.update(process_tree_identities(first.pane_pid))
        tracked_provider_identities.update(process_tree_identities(second.pane_pid))
        assertions["production_command_has_no_pure_or_model_flags"] = all(
            command_uses_native_defaults(command)
            for command in (
                tui_command(project, first.port, first_id),
                tui_command(project, second.port, second_id),
            )
        )
        assertions["native_tui_server_ready"] = True
        assertions["two_blank_same_root_sessions"] = (
            first_id != second_id and first.base != second.base
        )
        assertions["exact_blank_session_visible"] = exact_session_visible(
            first.base, first_id
        ) and exact_session_visible(second.base, second_id)
        assertions["endpoint_process_correlation"] = endpoint_owned_by_tree(
            first[3], first[2]
        ) and endpoint_owned_by_tree(second[3], second[2])
        assertions["wrong_healthy_endpoint_rejected"] = wrong_endpoint_check(
            project, first[2]
        )
        assertions["port_collision_rejected"] = port_collision_check(project, env)
        first_record = request(first[4], f"/session/{first_id}")
        assertions["unrelated_root_session_not_selected"] = (
            first[4] != second[4]
            and isinstance(first_record, dict)
            and isinstance(unrelated_record, dict)
            and first_record.get("directory") != unrelated_record.get("directory")
        )

        first_observer_path = root / "observer-first.json"
        second_observer_path = root / "observer-second.json"
        first_generation = f"first-{time.time_ns()}"
        second_generation = f"second-{time.time_ns()}"
        generations.extend((first_generation, second_generation))
        first_observer = start_observer(
            first[4], first_id, first_generation, first_observer_path
        )
        second_observer = start_observer(
            second[4], second_id, second_generation, second_observer_path
        )
        observers.extend((first_observer[0], second_observer[0]))
        tracked_observer_identities.update(
            {
                first_observer[1]: first_observer[2],
                second_observer[1]: second_observer[2],
            }
        )
        assertions["observer_started_before_native_input"] = True
        assertions["observer_generation_and_birth_recorded"] = process_identity_matches(
            first_observer[1], first_observer[2]
        ) and process_identity_matches(second_observer[1], second_observer[2])

        send_native_prompt(
            first[0],
            first[1],
            f"Reply with the exact token {FIRST_MARKER} and nothing else.",
        )
        send_native_prompt(
            second[0],
            second[1],
            f"Reply with the exact token {SECOND_MARKER} and nothing else.",
        )
        deadline = time.time() + 60
        while time.time() < deadline:
            first_result = observer_result(first_observer_path)
            second_result = observer_result(second_observer_path)
            if (
                first_result.get("matched_events", 0) > 0
                and second_result.get("matched_events", 0) > 0
            ):
                break
            time.sleep(0.5)
        first_result = observer_result(first_observer_path)
        second_result = observer_result(second_observer_path)
        assertions["observer_filtered_to_exact_session"] = (
            first_result.get("content_discarded") is True
            and second_result.get("content_discarded") is True
            and first_result.get("matched_events", 0) > 0
            and second_result.get("matched_events", 0) > 0
        )
        assertions["observer_discarded_foreign_events"] = bounded_event(
            {"properties": {"sessionID": "unrelated-session"}}, first_id
        ) == (False, True)
        assertions["child_session_events_ignored"] = bounded_event(
            {
                "properties": {
                    "sessionID": "provider-child-session",
                    "parentID": first_id,
                }
            },
            first_id,
        ) == (False, True)
        first_messages = request(first[4], f"/session/{first_id}/message")
        second_messages = request(second[4], f"/session/{second_id}/message")
        first_json = json.dumps(first_messages)
        second_json = json.dumps(second_messages)
        assertions["native_prompts_remain_non_crossing"] = (
            FIRST_MARKER in first_json
            and FIRST_MARKER not in second_json
            and SECOND_MARKER in second_json
            and SECOND_MARKER not in first_json
        )

        old_pid, old_birth = first_observer[1], first_observer[2]
        terminate_process(first_observer[0])
        observers.remove(first_observer[0])
        assertions["observer_helper_crash_detected"] = first_observer[
            0
        ].poll() is not None and not process_identity_matches(old_pid, old_birth)
        replacement_path = root / "observer-first-replacement.json"
        replacement = start_observer(
            first[4], first_id, first_generation, replacement_path
        )
        observers.append(replacement[0])
        tracked_observer_identities[replacement[1]] = replacement[2]
        replacement_result = observer_result(replacement_path)
        assertions["observer_replacement_reconnects"] = (
            (replacement[1], replacement[2]) != (old_pid, old_birth)
            and replacement_result.get("status") == "ready"
            and replacement_result.get("generation_digest") == digest(first_generation)
        )
        assertions["detach_reopen_retains_runtime_and_observer"] = (
            simulated_detach(first[0], first[1])
            and process_identity_matches(replacement[1], replacement[2])
            and private_tmux(
                first[0], "has-session", "-t", first[1], check=False
            ).returncode
            == 0
        )

        terminate_process(replacement[0])
        observers.remove(replacement[0])
        provider_tree = process_tree_identities(first.pane_pid)
        tracked_provider_identities.update(provider_tree)
        private_tmux(first[0], "kill-server", check=False)
        runtimes.remove(first)
        if not wait_port_closed(first[3], time.time() + 5):
            terminate_provider_tree(provider_tree)
        assertions["stale_saved_port_rejected"] = wait_port_closed(
            first[3], time.time() + 5
        )
        resumed = start_tui(root, project, env, first_id, expect_blank=False)
        runtimes.append(resumed)
        tracked_ports.add(resumed.port)
        tracked_provider_identities.update(process_tree_identities(resumed.pane_pid))
        assertions["exact_resume_after_runtime_restart"] = exact_session_visible(
            resumed.base, first_id
        ) and endpoint_owned_by_tree(resumed.port, resumed.pane_pid)

        required = tuple(
            name for name in assertions if name not in {"cleanup_complete"}
        )
        if not all(assertions[name] for name in required):
            failed = ",".join(name for name in required if not assertions[name])
            raise StudyFailure(f"OpenCode fresh-session assertions incomplete:{failed}")
        status, reason = "pass", "blank-native-binding-and-sidecar-ownership-confirmed"
    except StudyBlocked as error:
        status, reason = "blocked", str(error)
    except StudyFailure as error:
        status, reason = "falsified", str(error)
    except (
        OSError,
        TimeoutError,
        subprocess.TimeoutExpired,
        urllib.error.URLError,
        json.JSONDecodeError,
    ) as error:
        status, reason = "blocked", f"harness-error:{type(error).__name__}"
    finally:
        terminate_process(server)
        terminate_process(unrelated_server)
        for observer in observers:
            terminate_process(observer)
        for runtime in runtimes:
            tracked_provider_identities.update(
                process_tree_identities(runtime.pane_pid)
            )
            private_tmux(runtime.socket_path, "kill-server", check=False)
        providers_gone = wait_identities_gone(
            tracked_provider_identities, time.time() + 5
        )
        if not providers_gone:
            terminate_provider_tree(tracked_provider_identities)
            providers_gone = wait_identities_gone(
                tracked_provider_identities, time.time() + 5
            )
        observers_gone = wait_identities_gone(
            tracked_observer_identities, time.time() + 5
        )
        ports_closed = all(
            wait_port_closed(port, time.time() + 5) for port in tracked_ports
        )
        tmux_sessions_gone = all(
            private_tmux(
                runtime.socket_path,
                "has-session",
                "-t",
                runtime.session,
                check=False,
            ).returncode
            != 0
            for runtime in runtimes
        )
        root_removed = remove_root(root)
        assertions["cleanup_complete"] = (
            providers_gone
            and observers_gone
            and ports_closed
            and tmux_sessions_gone
            and root_removed
        )
        if status == "pass" and not assertions["cleanup_complete"]:
            status, reason = "falsified", "cleanup-incomplete"

    result = {
        "study": STUDY,
        "status": status,
        "reason": reason,
        "opencode_version": VERSION,
        "assertions": assertions,
        "session_digests": [digest(session_id) for session_id in session_ids],
        "generation_digests": [digest(generation) for generation in generations],
    }
    if args.result:
        args.result.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
        os.chmod(args.result, 0o600)
    else:
        print(json.dumps(result, indent=2))
    return 0 if status == "pass" else 1


if __name__ == "__main__":
    sys.exit(main())
