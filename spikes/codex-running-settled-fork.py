#!/usr/bin/env python3
"""Fork a native Codex TUI at its last settled turn while its tip is active."""

from __future__ import annotations

import argparse
import contextlib
import hashlib
import json
import os
import selectors
import shlex
import shutil
import subprocess
import sys
import tempfile
import time
from collections.abc import Mapping, Sequence
from pathlib import Path
from typing import Any, Self

STUDY = "codex-running-settled-fork"
SOURCE_SESSION = "wsnav-fork-source"
SOURCE_PRESENTATION = "wsnav-fork-source-presentation"
DESTINATION_SESSION = "wsnav-fork-destination"
DESTINATION_PRESENTATION = "wsnav-fork-destination-presentation"
BASELINE_MARKER = "WSNAV_FORK_BASELINE"
ALTERNATIVE_MARKER = "WSNAV_FORK_ALTERNATIVE"
BASELINE_PROMPT = (
    f"Reply with the exact token {BASELINE_MARKER} and nothing else. "
    "Do not use tools, inspect files, or make changes."
)
ACTIVE_PROMPT = (
    "Run the shell command sleep 120 exactly once. Do not edit files. "
    "After it finishes, reply with one short confirmation."
)
ALTERNATIVE_PROMPT = (
    "Run the shell command sleep 20 exactly once. Do not edit files. "
    f"After it finishes, reply with the exact token {ALTERNATIVE_MARKER} "
    "and nothing else."
)
TIMEOUT_SECONDS = 120.0
MAX_APP_SERVER_OUTPUT = 8 * 1024 * 1024
RECOVERY_SOURCE_KINDS = (
    "cli",
    "vscode",
    "exec",
    "appServer",
    "subAgent",
    "subAgentReview",
    "subAgentCompact",
    "subAgentThreadSpawn",
    "subAgentOther",
    "unknown",
)


class StudyFailure(RuntimeError):
    """The installed provider contradicted the settled-prefix fork design."""


class StudyBlocked(RuntimeError):
    """A live prerequisite was unavailable."""


class AppServerRejected(StudyFailure):
    """Codex rejected one App Server request."""


class AppServer:
    """One bounded JSONL connection to one short-lived Codex App Server."""

    def __init__(self, environment: Mapping[str, str]) -> None:
        self._stderr = tempfile.TemporaryFile(  # noqa: SIM115 - close owns it
            mode="w+b"
        )
        self._process = subprocess.Popen(
            ["codex", "app-server", "--listen", "stdio://"],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=self._stderr,
            env=dict(environment),
            bufsize=0,
            start_new_session=True,
        )
        if self._process.stdin is None or self._process.stdout is None:
            raise StudyFailure("app-server pipes were unavailable")
        self._selector = selectors.DefaultSelector()
        self._selector.register(self._process.stdout, selectors.EVENT_READ)
        self._buffer = b""
        self._received = 0
        self._next_id = 1
        self.closed_cleanly = False
        self.call(
            "initialize",
            {
                "clientInfo": {
                    "name": "workstream-navigator-spike",
                    "title": "Workstream Navigator Spike",
                    "version": "0",
                },
                "capabilities": {},
            },
        )
        self.send({"method": "initialized", "params": {}})

    def send(self, message: Mapping[str, Any]) -> None:
        if self._process.stdin is None:
            raise StudyFailure("app-server stdin was unavailable")
        payload = json.dumps(message, separators=(",", ":")).encode() + b"\n"
        try:
            self._process.stdin.write(payload)
            self._process.stdin.flush()
        except BrokenPipeError as error:
            raise StudyFailure("app-server closed stdin") from error

    def call(self, method: str, params: Mapping[str, Any]) -> dict[str, Any]:
        request_id = self._next_id
        self._next_id += 1
        self.send({"id": request_id, "method": method, "params": dict(params)})
        return self.receive_response(request_id, method)

    def receive_response(
        self,
        request_id: int,
        method: str,
    ) -> dict[str, Any]:
        deadline = time.monotonic() + 15.0
        while True:
            message = self._receive(deadline)
            if message.get("id") != request_id:
                continue
            if "error" in message:
                raise AppServerRejected(f"app-server rejected {method}")
            result = message.get("result")
            if not isinstance(result, dict):
                raise StudyFailure(f"app-server returned invalid {method} result")
            return result

    def send_without_response(
        self,
        method: str,
        params: Mapping[str, Any],
    ) -> int:
        request_id = self._next_id
        self._next_id += 1
        self.send({"id": request_id, "method": method, "params": dict(params)})
        return request_id

    def _receive(self, deadline: float) -> dict[str, Any]:
        if self._process.stdout is None:
            raise StudyFailure("app-server stdout was unavailable")
        while True:
            if b"\n" in self._buffer:
                line, self._buffer = self._buffer.split(b"\n", 1)
                if not line.strip():
                    continue
                try:
                    value = json.loads(line)
                except json.JSONDecodeError as error:
                    raise StudyFailure("app-server emitted invalid JSON") from error
                if not isinstance(value, dict):
                    raise StudyFailure("app-server emitted a non-object")
                return value
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise StudyFailure("app-server response timed out")
            events = self._selector.select(remaining)
            if not events:
                raise StudyFailure("app-server response timed out")
            chunk = os.read(self._process.stdout.fileno(), 65536)
            if not chunk:
                raise StudyFailure("app-server closed stdout")
            self._received += len(chunk)
            if self._received > MAX_APP_SERVER_OUTPUT:
                raise StudyFailure("app-server exceeded its output bound")
            self._buffer += chunk

    def close(self) -> None:
        try:
            if self._process.stdin is not None and not self._process.stdin.closed:
                with contextlib.suppress(BrokenPipeError):
                    self._process.stdin.close()
            try:
                self._process.wait(timeout=1.0)
            except subprocess.TimeoutExpired:
                self._process.terminate()
                try:
                    self._process.wait(timeout=1.0)
                except subprocess.TimeoutExpired:
                    self._process.kill()
                    self._process.wait(timeout=1.0)
            self.closed_cleanly = self._process.returncode == 0
        finally:
            self._selector.close()
            if self._process.stdout is not None:
                self._process.stdout.close()
            self._stderr.close()

    def __enter__(self) -> Self:
        return self

    def __exit__(
        self,
        _exception_type: object,
        _exception: object,
        _traceback: object,
    ) -> None:
        self.close()


def run(
    arguments: Sequence[str],
    *,
    environment: Mapping[str, str] | None = None,
    check: bool = True,
    timeout: float = 30.0,
) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        list(arguments),
        check=check,
        capture_output=True,
        text=True,
        env=None if environment is None else dict(environment),
        timeout=timeout,
    )


def private_tmux(
    socket: Path,
    *arguments: str,
    check: bool = True,
) -> subprocess.CompletedProcess[str]:
    environment = dict(os.environ)
    environment.pop("TMUX", None)
    return run(
        ["tmux", "-S", str(socket), *arguments],
        environment=environment,
        check=check,
    )


def ordinary_tmux_fingerprint() -> str:
    environment = dict(os.environ)
    environment.pop("TMUX", None)
    result = run(
        [
            "tmux",
            "list-sessions",
            "-F",
            "#{session_name}:#{session_created}:#{session_windows}",
            "-O",
            "name",
        ],
        environment=environment,
        check=False,
    )
    if result.returncode != 0:
        return "absent"
    return hashlib.sha256(result.stdout.encode()).hexdigest()


def agent_processes() -> dict[int, tuple[str, str]]:
    observed: dict[int, tuple[str, str]] = {}
    proc = Path("/proc")
    if not proc.is_dir():
        return observed
    for entry in proc.iterdir():
        if not entry.name.isdecimal():
            continue
        try:
            command = (entry / "comm").read_text(encoding="utf-8").strip()
            if command not in {"codex", "codex-cli"}:
                continue
            stat_fields = (entry / "stat").read_text(encoding="utf-8").split()
            cmdline = (entry / "cmdline").read_bytes().replace(b"\0", b" ").decode()
            observed[int(entry.name)] = (stat_fields[21], cmdline)
        except (FileNotFoundError, OSError, IndexError, UnicodeDecodeError):
            continue
    return observed


def preexisting_processes_unchanged(
    before: Mapping[int, tuple[str, str]],
) -> bool:
    after = agent_processes()
    return all(after.get(process_id) == facts for process_id, facts in before.items())


def process_birth(process_id: int) -> str:
    try:
        return Path(f"/proc/{process_id}/stat").read_text(encoding="utf-8").split()[21]
    except (FileNotFoundError, OSError, IndexError) as error:
        raise StudyFailure("native Codex process identity disappeared") from error


def process_parent(process_id: int) -> int | None:
    try:
        status = Path(f"/proc/{process_id}/status").read_text(encoding="utf-8")
    except (FileNotFoundError, OSError):
        return None
    for line in status.splitlines():
        if line.startswith("PPid:"):
            with contextlib.suppress(IndexError, ValueError):
                return int(line.split()[1])
    return None


def descendant_command_running(parent: int, command: str) -> bool:
    processes: dict[int, tuple[int, str]] = {}
    for entry in Path("/proc").iterdir():
        if not entry.name.isdecimal():
            continue
        parent_id = process_parent(int(entry.name))
        if parent_id is None:
            continue
        try:
            name = (entry / "comm").read_text(encoding="utf-8").strip()
        except (FileNotFoundError, OSError):
            continue
        processes[int(entry.name)] = (parent_id, name)
    descendants = {parent}
    changed = True
    while changed:
        changed = False
        for process_id, (parent_id, _name) in processes.items():
            if parent_id in descendants and process_id not in descendants:
                descendants.add(process_id)
                changed = True
    return any(
        process_id in descendants and name == command
        for process_id, (_parent_id, name) in processes.items()
    )


def descendant_command_cwd(parent: int, command: str) -> Path | None:
    processes: dict[int, tuple[int, str]] = {}
    for entry in Path("/proc").iterdir():
        if not entry.name.isdecimal():
            continue
        parent_id = process_parent(int(entry.name))
        if parent_id is None:
            continue
        try:
            name = (entry / "comm").read_text(encoding="utf-8").strip()
        except (FileNotFoundError, OSError):
            continue
        processes[int(entry.name)] = (parent_id, name)
    descendants = {parent}
    changed = True
    while changed:
        changed = False
        for process_id, (parent_id, _name) in processes.items():
            if parent_id in descendants and process_id not in descendants:
                descendants.add(process_id)
                changed = True
    for process_id, (_parent_id, name) in processes.items():
        if process_id not in descendants or name != command:
            continue
        try:
            return Path(f"/proc/{process_id}/cwd").resolve(strict=True)
        except (FileNotFoundError, OSError):
            continue
    return None


def capture(socket: Path, session: str) -> str:
    return private_tmux(
        socket,
        "capture-pane",
        "-p",
        "-t",
        f"{session}:0.0",
        "-S",
        "-220",
    ).stdout


def wait_for(predicate: Any, timeout: float = TIMEOUT_SECONDS) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if predicate():
            return
        time.sleep(0.2)
    raise StudyFailure("live provider observation timed out")


def send_line(socket: Path, session: str, value: str) -> None:
    private_tmux(socket, "send-keys", "-t", f"{session}:0.0", "-l", value)
    time.sleep(0.2)
    private_tmux(socket, "send-keys", "-t", f"{session}:0.0", "C-m")


def thread_value(result: Mapping[str, Any]) -> dict[str, Any]:
    thread = result.get("thread")
    if not isinstance(thread, dict):
        raise StudyFailure("app-server omitted the exact thread")
    return thread


def read_thread(
    environment: Mapping[str, str],
    thread_id: str,
    *,
    include_turns: bool,
) -> tuple[dict[str, Any], bool]:
    server = AppServer(environment)
    try:
        thread = thread_value(
            server.call(
                "thread/read",
                {"threadId": thread_id, "includeTurns": include_turns},
            )
        )
    finally:
        server.close()
    return thread, server.closed_cleanly


def list_threads(
    environment: Mapping[str, str],
    cwd: Path | None,
    *,
    source_kinds: Sequence[str] | None = ("cli",),
) -> tuple[list[dict[str, Any]], bool]:
    server = AppServer(environment)
    try:
        parameters: dict[str, Any] = {
            "archived": False,
            "limit": 20,
            "sortDirection": "desc",
            "sortKey": "created_at",
            "useStateDbOnly": True,
        }
        if cwd is not None:
            parameters["cwd"] = str(cwd)
        if source_kinds is not None:
            parameters["sourceKinds"] = list(source_kinds)
        result = server.call("thread/list", parameters)
        data = result.get("data")
        if not isinstance(data, list) or not all(
            isinstance(value, dict) for value in data
        ):
            raise StudyFailure("app-server returned invalid thread list")
    finally:
        server.close()
    return data, server.closed_cleanly


def completed_turns(thread: Mapping[str, Any]) -> list[dict[str, Any]]:
    turns = thread.get("turns")
    if not isinstance(turns, list):
        return []
    return [
        turn
        for turn in turns
        if isinstance(turn, dict) and turn.get("status") == "completed"
    ]


def in_progress_turns(thread: Mapping[str, Any]) -> list[dict[str, Any]]:
    turns = thread.get("turns")
    if not isinstance(turns, list):
        return []
    return [
        turn
        for turn in turns
        if isinstance(turn, dict) and turn.get("status") == "inProgress"
    ]


def identity(value: Mapping[str, Any], label: str) -> str:
    result = value.get("id")
    if not isinstance(result, str) or not result:
        raise StudyFailure(f"{label} identity was unavailable")
    return result


def schema_contract(
    environment: Mapping[str, str],
    root: Path,
) -> tuple[str, bool, bool]:
    schema_root = root / "schemas"
    schema_root.mkdir(mode=0o700)
    run(
        [
            "codex",
            "app-server",
            "generate-json-schema",
            "--out",
            str(schema_root),
        ],
        environment=environment,
    )
    documents: list[tuple[str, object]] = []
    for path in sorted(schema_root.glob("*.json")):
        documents.append((path.name, json.loads(path.read_text(encoding="utf-8"))))
    if not documents:
        raise StudyFailure("Codex generated no App Server schemas")
    client_request = next(
        (
            document
            for name, document in documents
            if name == "ClientRequest.json" and isinstance(document, dict)
        ),
        None,
    )
    definitions = (
        client_request.get("definitions")
        if isinstance(client_request, Mapping)
        else None
    )
    fork_params = (
        definitions.get("ThreadForkParams")
        if isinstance(definitions, Mapping)
        else None
    )
    properties = (
        fork_params.get("properties") if isinstance(fork_params, Mapping) else None
    )
    required = fork_params.get("required") if isinstance(fork_params, Mapping) else None
    exact_boundary_available = (
        isinstance(properties, Mapping)
        and {"threadId", "lastTurnId", "cwd"}.issubset(properties)
        and isinstance(required, list)
        and "threadId" in required
    )
    idempotency_absent = (
        isinstance(properties, Mapping) and "idempotencyKey" not in properties
    )
    encoded = json.dumps(
        documents,
        ensure_ascii=True,
        allow_nan=False,
        separators=(",", ":"),
        sort_keys=True,
    ).encode()
    return (
        f"sha256:{hashlib.sha256(encoded).hexdigest()}",
        exact_boundary_available,
        idempotency_absent,
    )


def write_config(
    codex_home: Path,
    source_checkout: Path,
    destination_checkout: Path,
) -> None:
    config = (
        'model_reasoning_effort = "low"\n\n'
        "[features]\n"
        "hooks = false\n\n"
        f'[projects."{source_checkout}"]\n'
        'trust_level = "trusted"\n\n'
        f'[projects."{destination_checkout}"]\n'
        'trust_level = "trusted"\n'
    )
    path = codex_home / "config.toml"
    path.write_text(config, encoding="utf-8")
    path.chmod(0o600)


def write_tmux_config(path: Path) -> None:
    path.write_text(
        'set -g default-terminal "tmux-256color"\n'
        "set -g status off\n"
        "set -g mouse on\n"
        "set-environment -g COLORTERM truecolor\n",
        encoding="utf-8",
    )
    path.chmod(0o600)


def start_tui(
    *,
    runtime_socket: Path,
    presentation_socket: Path,
    tmux_config: Path,
    runtime_session: str,
    presentation_session: str,
    codex_home: Path,
    cwd: Path,
    resume_thread_id: str | None,
) -> tuple[int, tuple[str, str, str, str]]:
    codex_arguments = [
        "codex",
        "-s",
        "read-only",
        "-a",
        "never",
        "-C",
        str(cwd),
    ]
    if resume_thread_id is not None:
        codex_arguments.extend(["resume", resume_thread_id])
    runtime_command = "exec env " + " ".join(
        [
            f"CODEX_HOME={shlex.quote(str(codex_home))}",
            "COLORTERM=truecolor",
            *(shlex.quote(value) for value in codex_arguments),
        ]
    )
    private_tmux(
        runtime_socket,
        "-f",
        str(tmux_config),
        "new-session",
        "-d",
        "-s",
        runtime_session,
        "-x",
        "120",
        "-y",
        "42",
        runtime_command,
    )
    attach_command = (
        "exec env -u TMUX tmux -S "
        f"{shlex.quote(str(runtime_socket))} attach-session -t {runtime_session}"
    )
    private_tmux(
        presentation_socket,
        "-f",
        str(tmux_config),
        "new-session",
        "-d",
        "-s",
        presentation_session,
        "-x",
        "140",
        "-y",
        "44",
        attach_command,
    )
    wait_for(
        lambda: "OpenAI Codex" in capture(presentation_socket, presentation_session)
    )
    output = private_tmux(
        runtime_socket,
        "display-message",
        "-p",
        "-t",
        f"{runtime_session}:0.0",
        "#{pane_pid}\t#{pane_id}\t#{pane_current_path}",
    ).stdout.rstrip("\n")
    values = output.split("\t")
    if len(values) != 3 or not values[0].isdecimal():
        raise StudyFailure("native TUI facts were malformed")
    process_id = int(values[0])
    return process_id, (
        values[0],
        values[1],
        values[2],
        process_birth(process_id),
    )


def exact_thread_for_cwd(
    environment: Mapping[str, str],
    cwd: Path,
) -> tuple[str, bool]:
    helpers_closed = True
    deadline = time.monotonic() + TIMEOUT_SECONDS
    while time.monotonic() < deadline:
        threads, closed = list_threads(environment, cwd)
        helpers_closed = helpers_closed and closed
        if len(threads) == 1:
            return identity(threads[0], "source thread"), helpers_closed
        time.sleep(0.25)
    raise StudyFailure("exact native source thread was not identified")


def wait_for_completed_turn(
    environment: Mapping[str, str],
    thread_id: str,
    expected_count: int,
) -> tuple[dict[str, Any], bool]:
    helpers_closed = True
    deadline = time.monotonic() + TIMEOUT_SECONDS
    while time.monotonic() < deadline:
        thread, closed = read_thread(environment, thread_id, include_turns=True)
        helpers_closed = helpers_closed and closed
        if len(completed_turns(thread)) >= expected_count:
            return thread, helpers_closed
        time.sleep(0.25)
    raise StudyFailure("native turn did not settle")


def write_result(path: Path, value: Mapping[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(f".{path.name}.tmp")
    temporary.write_text(
        json.dumps(value, indent=2, sort_keys=False) + "\n",
        encoding="utf-8",
    )
    temporary.chmod(0o600)
    temporary.replace(path)


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--result", required=True, type=Path)
    parser.add_argument("--debug", action="store_true")
    arguments = parser.parse_args(argv)

    started = time.monotonic()
    version = "unknown"
    fingerprint = "settled-prefix-fork-v1"
    status = "falsified"
    reason = "provider-contract-not-proven"
    assertions: dict[str, bool] = {}
    cleanup = "incomplete"
    failure_stage = "prerequisites"
    root: Path | None = None
    sockets: list[Path] = []
    ordinary_tmux_before = ordinary_tmux_fingerprint()
    preexisting_agents = agent_processes()
    helpers_closed = True

    try:
        for command in ("codex", "git", "tmux"):
            if shutil.which(command) is None:
                raise StudyBlocked(f"{command} unavailable")
        source_auth = Path.home() / ".codex" / "auth.json"
        if not source_auth.is_file():
            raise StudyBlocked("Codex auth unavailable")
        version_output = run(["codex", "--version"]).stdout.strip().split()
        if len(version_output) < 2:
            raise StudyFailure("Codex version was malformed")
        version = version_output[1]

        root = Path(tempfile.mkdtemp(prefix="wsnav-codex-fork-spike."))
        failure_stage = "isolated-setup"
        root.chmod(0o700)
        codex_home = root / "codex-home"
        source_checkout = root / "source"
        destination_checkout = root / "destination"
        codex_home.mkdir(mode=0o700)
        source_checkout.mkdir(mode=0o700)
        shutil.copyfile(source_auth, codex_home / "auth.json")
        (codex_home / "auth.json").chmod(0o600)

        run(["git", "-C", str(source_checkout), "init", "-q", "-b", "main"])
        run(
            [
                "git",
                "-C",
                str(source_checkout),
                "config",
                "user.email",
                "wsnav-spike@example.invalid",
            ]
        )
        run(
            [
                "git",
                "-C",
                str(source_checkout),
                "config",
                "user.name",
                "Workstream Navigator Spike",
            ]
        )
        seed = source_checkout / "seed.txt"
        seed.write_text("disposable fork spike\n", encoding="utf-8")
        run(["git", "-C", str(source_checkout), "add", "seed.txt"])
        run(
            [
                "git",
                "-C",
                str(source_checkout),
                "commit",
                "-q",
                "-m",
                "spike seed",
            ]
        )
        base_commit = run(
            ["git", "-C", str(source_checkout), "rev-parse", "HEAD"]
        ).stdout.strip()

        write_config(codex_home, source_checkout, destination_checkout)
        tmux_config = root / "tmux.conf"
        write_tmux_config(tmux_config)
        source_runtime_socket = root / "source-runtime.sock"
        source_presentation_socket = root / "source-presentation.sock"
        destination_runtime_socket = root / "destination-runtime.sock"
        destination_presentation_socket = root / "destination-presentation.sock"
        sockets.extend(
            [
                source_runtime_socket,
                source_presentation_socket,
                destination_runtime_socket,
                destination_presentation_socket,
            ]
        )

        provider_environment = dict(os.environ)
        provider_environment["CODEX_HOME"] = str(codex_home)
        provider_environment["COLORTERM"] = "truecolor"
        (
            fingerprint,
            exact_boundary_available,
            idempotency_absent,
        ) = schema_contract(provider_environment, root)

        failure_stage = "source-tui-start"
        source_pid, source_facts = start_tui(
            runtime_socket=source_runtime_socket,
            presentation_socket=source_presentation_socket,
            tmux_config=tmux_config,
            runtime_session=SOURCE_SESSION,
            presentation_session=SOURCE_PRESENTATION,
            codex_home=codex_home,
            cwd=source_checkout,
            resume_thread_id=None,
        )

        send_line(
            source_presentation_socket,
            SOURCE_PRESENTATION,
            BASELINE_PROMPT,
        )
        failure_stage = "baseline-turn"
        source_thread_id, closed = exact_thread_for_cwd(
            provider_environment,
            source_checkout,
        )
        helpers_closed = helpers_closed and closed
        source_after_x, closed = wait_for_completed_turn(
            provider_environment,
            source_thread_id,
            1,
        )
        helpers_closed = helpers_closed and closed
        baseline_turns = completed_turns(source_after_x)
        if len(baseline_turns) != 1:
            raise StudyFailure("baseline settled boundary was ambiguous")
        baseline_turn_id = identity(baseline_turns[0], "baseline turn")
        wait_for(
            lambda: (
                "esc to interrupt"
                not in capture(source_presentation_socket, SOURCE_PRESENTATION).lower()
            )
        )

        send_line(
            source_presentation_socket,
            SOURCE_PRESENTATION,
            ACTIVE_PROMPT,
        )
        failure_stage = "source-active-turn"
        wait_for(lambda: descendant_command_running(source_pid, "sleep"))
        source_during_y, closed = read_thread(
            provider_environment,
            source_thread_id,
            include_turns=True,
        )
        helpers_closed = helpers_closed and closed
        persisted_turns_during_y = source_during_y.get("turns")
        persisted_statuses_during_y = (
            [
                turn.get("status")
                for turn in persisted_turns_during_y
                if isinstance(turn, Mapping)
            ]
            if isinstance(persisted_turns_during_y, list)
            else []
        )
        helper_status_is_not_live_authority = persisted_statuses_during_y == [
            "completed",
            "interrupted",
        ] and descendant_command_running(source_pid, "sleep")
        if not helper_status_is_not_live_authority:
            raise StudyFailure("persisted active-turn observation was unexpected")
        source_facts_before_fork = (
            source_facts[0],
            source_facts[1],
            source_facts[2],
            process_birth(source_pid),
        )

        run(
            [
                "git",
                "-C",
                str(source_checkout),
                "worktree",
                "add",
                "-q",
                "-b",
                "wsnav-spike-destination",
                str(destination_checkout),
                base_commit,
            ]
        )
        failure_stage = "destination-worktree"
        destination_head = run(
            ["git", "-C", str(destination_checkout), "rev-parse", "HEAD"]
        ).stdout.strip()

        operation_started_at = int(time.time())
        failure_stage = "lost-response-submit"
        valid_fork_submission_count = 0
        lost_response_server = AppServer(provider_environment)
        lost_response_request_id = lost_response_server.send_without_response(
            "thread/fork",
            {
                "approvalPolicy": "never",
                "cwd": str(destination_checkout),
                "lastTurnId": baseline_turn_id,
                "sandbox": "read-only",
                "threadSource": "appServer",
                "threadId": source_thread_id,
            },
        )
        valid_fork_submission_count += 1
        debug_response_observed = False
        if arguments.debug:
            debug_fork_result = lost_response_server.receive_response(
                lost_response_request_id,
                "thread/fork",
            )
            debug_fork_thread = thread_value(debug_fork_result)
            debug_response_observed = True
            print(
                "lost-response-submit: "
                + json.dumps(
                    {
                        "provider_returned_success": True,
                        "cwd_matches": (
                            debug_fork_thread.get("cwd") == str(destination_checkout)
                        ),
                        "lineage_matches": (
                            debug_fork_thread.get("forkedFromId") == source_thread_id
                        ),
                        "source_kind": debug_fork_thread.get("source"),
                        "turn_count": (
                            len(debug_fork_thread.get("turns"))
                            if isinstance(debug_fork_thread.get("turns"), list)
                            else -1
                        ),
                    },
                    sort_keys=True,
                ),
                file=sys.stderr,
            )
        else:
            time.sleep(1.0)
        lost_response_server.close()
        helpers_closed = helpers_closed and lost_response_server.closed_cleanly

        reconciled_candidates: list[dict[str, Any]] = []
        failure_stage = "lost-response-reconcile"
        destination_thread_id: str | None = None
        reconcile_diagnostics: list[dict[str, Any]] = []
        deadline = time.monotonic() + 30.0
        while time.monotonic() < deadline:
            listed, closed = list_threads(
                provider_environment,
                None,
                source_kinds=RECOVERY_SOURCE_KINDS,
            )
            helpers_closed = helpers_closed and closed
            candidates: list[dict[str, Any]] = []
            reconcile_diagnostics = []
            for listed_thread in listed:
                candidate_id = listed_thread.get("id")
                if not isinstance(candidate_id, str):
                    continue
                candidate, closed = read_thread(
                    provider_environment,
                    candidate_id,
                    include_turns=True,
                )
                helpers_closed = helpers_closed and closed
                candidate_turns = candidate.get("turns")
                created_at = candidate.get("createdAt")
                reconcile_diagnostics.append(
                    {
                        "source_kind": candidate.get("source"),
                        "cwd_matches": (
                            candidate.get("cwd") == str(destination_checkout)
                        ),
                        "lineage_matches": (
                            candidate.get("forkedFromId") == source_thread_id
                        ),
                        "created_at_is_integer": isinstance(created_at, int),
                        "created_recently": (
                            isinstance(created_at, int)
                            and created_at >= operation_started_at - 1
                        ),
                        "turn_count": (
                            len(candidate_turns)
                            if isinstance(candidate_turns, list)
                            else -1
                        ),
                        "turn_statuses": (
                            [
                                turn.get("status")
                                for turn in candidate_turns
                                if isinstance(turn, Mapping)
                            ]
                            if isinstance(candidate_turns, list)
                            else []
                        ),
                        "boundary_matches": (
                            isinstance(candidate_turns, list)
                            and len(candidate_turns) == 1
                            and isinstance(candidate_turns[0], Mapping)
                            and candidate_turns[0].get("id") == baseline_turn_id
                        ),
                    }
                )
                if (
                    candidate.get("forkedFromId") == source_thread_id
                    and isinstance(created_at, int)
                    and created_at >= operation_started_at - 1
                    and isinstance(candidate_turns, list)
                    and len(candidate_turns) == 1
                    and isinstance(candidate_turns[0], Mapping)
                    and candidate_turns[0].get("id") == baseline_turn_id
                    and candidate_turns[0].get("status") == "completed"
                ):
                    candidates.append(candidate)
            if len(candidates) == 1:
                reconciled_candidates = candidates
                destination_thread_id = identity(candidates[0], "destination thread")
                break
            if len(candidates) > 1:
                raise StudyFailure("lost-response reconciliation was ambiguous")
            time.sleep(0.25)
        if destination_thread_id is None:
            raise StudyFailure(
                "lost-response fork could not be reconciled: "
                f"observed={json.dumps(reconcile_diagnostics, sort_keys=True)}"
            )
        cli_only_threads, closed = list_threads(
            provider_environment,
            None,
        )
        helpers_closed = helpers_closed and closed
        destination_absent_from_cli_only_recovery = all(
            thread.get("id") != destination_thread_id for thread in cli_only_threads
        )

        destination_before_z = reconciled_candidates[0]
        failure_stage = "destination-resume"
        destination_turns_before_z = destination_before_z.get("turns")
        source_sleep_running_after_fork = descendant_command_running(
            source_pid, "sleep"
        )

        destination_pid, _destination_facts = start_tui(
            runtime_socket=destination_runtime_socket,
            presentation_socket=destination_presentation_socket,
            tmux_config=tmux_config,
            runtime_session=DESTINATION_SESSION,
            presentation_session=DESTINATION_PRESENTATION,
            codex_home=codex_home,
            cwd=destination_checkout,
            resume_thread_id=destination_thread_id,
        )
        wait_for(
            lambda: (
                BASELINE_MARKER
                in capture(
                    destination_presentation_socket,
                    DESTINATION_PRESENTATION,
                )
            )
        )
        send_line(
            destination_presentation_socket,
            DESTINATION_PRESENTATION,
            ALTERNATIVE_PROMPT,
        )
        failure_stage = "destination-divergent-turn"
        wait_for(lambda: descendant_command_running(destination_pid, "sleep"))
        destination_command_cwd = descendant_command_cwd(
            destination_pid,
            "sleep",
        )
        destination_after_z, closed = wait_for_completed_turn(
            provider_environment,
            destination_thread_id,
            2,
        )
        helpers_closed = helpers_closed and closed

        source_facts_after_z_output = private_tmux(
            source_runtime_socket,
            "display-message",
            "-p",
            "-t",
            f"{SOURCE_SESSION}:0.0",
            "#{pane_pid}\t#{pane_id}\t#{pane_current_path}",
        ).stdout.rstrip("\n")
        source_facts_after_z_values = source_facts_after_z_output.split("\t")
        if len(source_facts_after_z_values) != 3:
            raise StudyFailure("source TUI facts changed shape")
        source_facts_after_z = (
            source_facts_after_z_values[0],
            source_facts_after_z_values[1],
            source_facts_after_z_values[2],
            process_birth(source_pid),
        )

        destination_completed = completed_turns(destination_after_z)
        failure_stage = "final-assertions"
        assertions = {
            "exact_fork_boundary_in_schema": exact_boundary_available,
            "fork_has_no_idempotency_key": idempotency_absent,
            "baseline_turn_settled_before_source_continued": (len(baseline_turns) == 1),
            "source_next_turn_observed_running": descendant_command_running(
                source_pid, "sleep"
            ),
            "ephemeral_status_not_used_as_live_authority": (
                helper_status_is_not_live_authority
            ),
            "default_base_commit_recorded": bool(base_commit),
            "destination_worktree_uses_recorded_base": (
                destination_head == base_commit
            ),
            "valid_fork_submitted_exactly_once": (valid_fork_submission_count == 1),
            "fork_response_intentionally_discarded": True,
            "debug_response_not_used": not debug_response_observed,
            "lost_response_reconciled_one_destination": (
                len(reconciled_candidates) == 1
            ),
            "non_cli_source_kind_included_for_fork_recovery": (
                destination_absent_from_cli_only_recovery
            ),
            "provider_records_source_lineage": (
                destination_before_z.get("forkedFromId") == source_thread_id
            ),
            "destination_contains_only_settled_prefix": (
                isinstance(destination_turns_before_z, list)
                and len(destination_turns_before_z) == 1
                and isinstance(destination_turns_before_z[0], Mapping)
                and destination_turns_before_z[0].get("id") == baseline_turn_id
                and destination_turns_before_z[0].get("status") == "completed"
            ),
            "source_active_turn_excluded_from_destination": (
                isinstance(destination_turns_before_z, list)
                and len(destination_turns_before_z) == 1
            ),
            "source_turn_remains_in_progress_after_fork": (
                source_sleep_running_after_fork
            ),
            "destination_resumes_in_separate_native_tui": (
                destination_thread_id != source_thread_id
                and destination_pid != source_pid
            ),
            "fork_cwd_deferred_to_native_resume": (
                destination_before_z.get("cwd") != str(destination_checkout)
                and destination_command_cwd == destination_checkout
            ),
            "destination_provider_command_uses_worktree": (
                destination_command_cwd == destination_checkout
            ),
            "destination_native_history_shows_settled_prefix": (
                BASELINE_MARKER
                in capture(
                    destination_presentation_socket,
                    DESTINATION_PRESENTATION,
                )
            ),
            "destination_accepts_divergent_native_turn": (
                len(destination_completed) == 2
                and destination_completed[0].get("id") == baseline_turn_id
                and destination_completed[1].get("id") != baseline_turn_id
            ),
            "source_still_running_after_destination_diverges": (
                descendant_command_running(source_pid, "sleep")
            ),
            "source_native_runtime_facts_stable": (
                source_facts_before_fork == source_facts_after_z
            ),
            "helpers_exit_after_operations": helpers_closed,
            "source_checkout_clean": not run(
                ["git", "-C", str(source_checkout), "status", "--porcelain"]
            ).stdout,
            "destination_checkout_clean": not run(
                ["git", "-C", str(destination_checkout), "status", "--porcelain"]
            ).stdout,
        }
        if not all(assertions.values()):
            raise StudyFailure("one or more fork assertions failed")

        send_line(
            destination_presentation_socket,
            DESTINATION_PRESENTATION,
            "/exit",
        )
        status = "pass"
        reason = "running-source-settled-prefix-fork-proven"
    except StudyBlocked:
        status = "blocked"
        reason = "live-provider-prerequisite-unavailable"
    except (
        OSError,
        StudyFailure,
        subprocess.SubprocessError,
        ValueError,
    ) as error:
        status = "falsified"
        reason = "provider-contract-not-proven"
        if arguments.debug:
            print(
                f"{failure_stage}: {type(error).__name__}: {error}",
                file=sys.stderr,
            )
    finally:
        for socket in reversed(sockets):
            private_tmux(socket, "kill-server", check=False)
        if root is not None:
            root_string = str(root)
            if root_string.startswith(
                "/tmp/wsnav-codex-fork-spike."
            ) and root.parent == Path("/tmp"):
                shutil.rmtree(root, ignore_errors=True)
        time.sleep(0.75)
        root_deleted = root is None or not root.exists()
        ordinary_tmux_unchanged = ordinary_tmux_fingerprint() == ordinary_tmux_before
        unrelated_agents_unchanged = preexisting_processes_unchanged(preexisting_agents)
        if status == "pass" and not (
            root_deleted and ordinary_tmux_unchanged and unrelated_agents_unchanged
        ):
            status = "falsified"
            reason = "cleanup-or-isolation-verification-failed"
        cleanup = (
            "complete"
            if root_deleted and ordinary_tmux_unchanged
            else "verification-failed"
        )
        assertions["temporary_root_deleted"] = root_deleted
        assertions["ordinary_tmux_unchanged"] = ordinary_tmux_unchanged
        assertions["preexisting_agent_processes_unchanged"] = unrelated_agents_unchanged

    result = {
        "study": STUDY,
        "provider": {
            "id": "codex",
            "version": version,
            "contract_fingerprint": fingerprint,
        },
        "status": status,
        "reason": reason,
        "failure_stage": None if status == "pass" else failure_stage,
        "assertions": assertions,
        "cleanup": cleanup,
        "elapsed_seconds": int(time.monotonic() - started),
    }
    write_result(arguments.result.resolve(), result)
    return 0 if status == "pass" else 1


if __name__ == "__main__":
    raise SystemExit(main())
