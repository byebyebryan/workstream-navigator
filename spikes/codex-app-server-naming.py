#!/usr/bin/env python3
"""Isolated Codex App Server metadata and naming-boundary spike."""

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
import tempfile
import time
from collections.abc import Mapping, Sequence
from pathlib import Path
from typing import Any, Self

STUDY = "codex-app-server-naming"
CONTRACT = "ephemeral-stdio-thread-metadata-v1"
RUNTIME_SESSION = "wsnav-metadata-runtime"
PRESENTATION_SESSION = "wsnav-metadata-presentation"
BASELINE_MARKER = "WSNAV_METADATA_BASELINE"
BASELINE_PROMPT = (
    f"Reply with the exact token {BASELINE_MARKER} and nothing else. "
    "Do not use tools, inspect files, or make changes."
)
NATIVE_NAME = "wsnav-native-metadata-probe"
APP_SERVER_NAME = "wsnav-app-server-metadata-probe"
TIMEOUT_SECONDS = 90.0
MAX_APP_SERVER_OUTPUT = 8 * 1024 * 1024


class StudyFailure(RuntimeError):
    """The installed provider contract contradicted the V1 design."""


class StudyBlocked(RuntimeError):
    """The live provider contract could not be exercised."""


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
        deadline = time.monotonic() + 15.0
        while True:
            message = self._receive(deadline)
            if message.get("id") != request_id:
                continue
            if "error" in message:
                raise StudyFailure(f"app-server rejected {method}")
            result = message.get("result")
            if not isinstance(result, dict):
                raise StudyFailure(f"app-server returned invalid {method} result")
            return result

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


def capture(socket: Path, session: str) -> str:
    return private_tmux(
        socket,
        "capture-pane",
        "-p",
        "-t",
        f"{session}:0.0",
        "-S",
        "-180",
    ).stdout


def wait_for(predicate: Any, timeout: float = TIMEOUT_SECONDS) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if predicate():
            return
        time.sleep(0.2)
    raise StudyFailure("native TUI observation timed out")


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
    workspace: Path,
) -> tuple[list[dict[str, Any]], bool]:
    server = AppServer(environment)
    try:
        result = server.call(
            "thread/list",
            {
                "archived": False,
                "cwd": str(workspace),
                "limit": 20,
                "sortDirection": "desc",
                "sortKey": "recency_at",
                "sourceKinds": ["cli"],
                "useStateDbOnly": True,
            },
        )
        data = result.get("data")
        if not isinstance(data, list) or not all(
            isinstance(value, dict) for value in data
        ):
            raise StudyFailure("app-server returned invalid thread list")
    finally:
        server.close()
    return data, server.closed_cleanly


def set_name(
    environment: Mapping[str, str],
    thread_id: str,
    name: str,
) -> bool:
    server = AppServer(environment)
    try:
        server.call(
            "thread/name/set",
            {"threadId": thread_id, "name": name},
        )
    finally:
        server.close()
    return server.closed_cleanly


def schema_contract(environment: Mapping[str, str], root: Path) -> tuple[str, bool]:
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
    set_name_params = (
        definitions.get("ThreadSetNameParams")
        if isinstance(definitions, Mapping)
        else None
    )
    properties = (
        set_name_params.get("properties")
        if isinstance(set_name_params, Mapping)
        else None
    )
    name_set_has_no_compare_and_set = isinstance(properties, Mapping) and set(
        properties
    ) == {"name", "threadId"}
    encoded = json.dumps(
        documents,
        ensure_ascii=True,
        allow_nan=False,
        separators=(",", ":"),
        sort_keys=True,
    ).encode()
    return (
        f"sha256:{hashlib.sha256(encoded).hexdigest()}",
        name_set_has_no_compare_and_set,
    )


def settled_thread(thread: Mapping[str, Any]) -> bool:
    turns = thread.get("turns")
    return isinstance(turns, list) and any(
        isinstance(turn, Mapping) and turn.get("status") == "completed"
        for turn in turns
    )


def resolve_name(
    *,
    state: str,
    workstream: str,
    native_name: str | None = None,
    cached_name: str | None = None,
    context: str = "existing",
    previous_name: str | None = None,
    source_name: str | None = None,
    source_workstream: str | None = None,
) -> tuple[str, str]:
    if state == "named" and native_name:
        return native_name, "native"
    if state == "unavailable" and cached_name:
        return f"{cached_name} · stale", "cached_stale"
    if context == "cutover":
        if state == "unavailable":
            prefix = previous_name or f"untitled · {workstream}"
            return f"{prefix} ↻ name unavailable", "cutover_fallback"
        if previous_name:
            return f"{previous_name} ↻ unnamed", "cutover_fallback"
        return f"untitled · {workstream} ↻", "cutover_fallback"
    if context == "fork":
        if state == "unavailable":
            prefix = source_name or f"fork of {source_workstream}"
            return f"{prefix} · name unavailable", "fork_fallback"
        if source_name:
            return f"{source_name} · fork · {workstream}", "fork_fallback"
        return f"fork of {source_workstream} · {workstream}", "fork_fallback"
    if state == "known_empty":
        return f"untitled · {workstream}", "synthetic"
    return f"name unavailable · {workstream}", "synthetic"


def metadata_filter(thread: Mapping[str, Any]) -> dict[str, str | None]:
    name = thread.get("name")
    if isinstance(name, str) and name.strip():
        return {"name_state": "named", "name": name}
    return {"name_state": "known_empty", "name": None}


def stdio_command_allowed(arguments: Sequence[str]) -> bool:
    values = list(arguments)
    return values in (
        ["codex", "app-server", "--listen", "stdio://"],
        ["codex", "app-server", "--stdio"],
    )


def managed_runtime_allowed(arguments: Sequence[str]) -> bool:
    return "--remote" not in arguments and not any(
        argument.startswith("--remote=") for argument in arguments
    )


def write_config(codex_home: Path, workspace: Path) -> None:
    config = (
        'model_reasoning_effort = "low"\n\n'
        "[features]\n"
        "hooks = false\n\n"
        f'[projects."{workspace}"]\n'
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
    arguments = parser.parse_args(argv)

    started = time.monotonic()
    version = "unknown"
    fingerprint = CONTRACT
    status = "falsified"
    reason = "provider-contract-not-proven"
    assertions: dict[str, bool] = {}
    cleanup = "incomplete"
    root: Path | None = None
    runtime_socket: Path | None = None
    presentation_socket: Path | None = None
    runtime_started = False
    presentation_started = False
    ordinary_tmux_before = ordinary_tmux_fingerprint()
    preexisting_agents = agent_processes()

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

        root = Path(tempfile.mkdtemp(prefix="wsnav-codex-metadata-spike."))
        root.chmod(0o700)
        codex_home = root / "codex-home"
        workspace = root / "workspace"
        codex_home.mkdir(mode=0o700)
        workspace.mkdir(mode=0o700)
        shutil.copyfile(source_auth, codex_home / "auth.json")
        (codex_home / "auth.json").chmod(0o600)
        run(["git", "-C", str(workspace), "init", "-q"])
        write_config(codex_home, workspace)
        tmux_config = root / "tmux.conf"
        write_tmux_config(tmux_config)
        runtime_socket = root / "runtime.sock"
        presentation_socket = root / "presentation.sock"

        provider_environment = dict(os.environ)
        provider_environment["CODEX_HOME"] = str(codex_home)
        provider_environment["COLORTERM"] = "truecolor"
        fingerprint, name_set_has_no_compare_and_set = schema_contract(
            provider_environment, root
        )

        runtime_command = "exec env " + " ".join(
            (
                f"CODEX_HOME={shlex.quote(str(codex_home))}",
                "COLORTERM=truecolor",
                "codex",
                "-s",
                "read-only",
                "-a",
                "never",
                "-C",
                shlex.quote(str(workspace)),
            )
        )
        private_tmux(
            runtime_socket,
            "-f",
            str(tmux_config),
            "new-session",
            "-d",
            "-s",
            RUNTIME_SESSION,
            "-x",
            "120",
            "-y",
            "42",
            runtime_command,
        )
        runtime_started = True
        attach_command = (
            "exec env -u TMUX tmux -S "
            f"{shlex.quote(str(runtime_socket))} attach-session -t {RUNTIME_SESSION}"
        )
        private_tmux(
            presentation_socket,
            "-f",
            str(tmux_config),
            "new-session",
            "-d",
            "-s",
            PRESENTATION_SESSION,
            "-x",
            "140",
            "-y",
            "44",
            attach_command,
        )
        presentation_started = True
        wait_for(
            lambda: "OpenAI Codex" in capture(presentation_socket, PRESENTATION_SESSION)
        )

        facts_output = private_tmux(
            runtime_socket,
            "display-message",
            "-p",
            "-t",
            f"{RUNTIME_SESSION}:0.0",
            "#{pane_pid}\t#{pane_id}\t#{pane_current_path}",
        ).stdout.rstrip("\n")
        facts = facts_output.split("\t")
        if len(facts) != 3 or not facts[0].isdecimal():
            raise StudyFailure("native TUI facts were malformed")
        provider_pid = int(facts[0])
        provider_birth = process_birth(provider_pid)
        provider_facts = (facts[0], facts[1], facts[2], provider_birth)

        send_line(presentation_socket, PRESENTATION_SESSION, BASELINE_PROMPT)
        wait_for(
            lambda: (
                "esc to interrupt"
                in capture(presentation_socket, PRESENTATION_SESSION).lower()
            ),
            timeout=20.0,
        )

        thread_id: str | None = None
        helpers_closed = True
        deadline = time.monotonic() + TIMEOUT_SECONDS
        while time.monotonic() < deadline:
            listed, closed = list_threads(provider_environment, workspace)
            helpers_closed = helpers_closed and closed
            if len(listed) == 1 and isinstance(listed[0].get("id"), str):
                candidate = str(listed[0]["id"])
                observed, closed = read_thread(
                    provider_environment,
                    candidate,
                    include_turns=True,
                )
                helpers_closed = helpers_closed and closed
                if settled_thread(observed):
                    thread_id = candidate
                    break
            time.sleep(0.25)
        if thread_id is None:
            raise StudyFailure("settled native thread was not identified")
        wait_for(
            lambda: (
                "esc to interrupt"
                not in capture(presentation_socket, PRESENTATION_SESSION).lower()
            )
        )

        screen_before_read = capture(runtime_socket, RUNTIME_SESSION)
        facts_before_read = (
            facts[0],
            facts[1],
            facts[2],
            process_birth(provider_pid),
        )
        initial_thread, closed = read_thread(
            provider_environment,
            thread_id,
            include_turns=False,
        )
        helpers_closed = helpers_closed and closed
        filtered = metadata_filter(initial_thread)
        screen_after_read = capture(runtime_socket, RUNTIME_SESSION)

        send_line(
            presentation_socket,
            PRESENTATION_SESSION,
            f"/rename {NATIVE_NAME}",
        )
        native_name_visible = False
        deadline = time.monotonic() + 15.0
        while time.monotonic() < deadline:
            observed, closed = read_thread(
                provider_environment,
                thread_id,
                include_turns=False,
            )
            helpers_closed = helpers_closed and closed
            if observed.get("name") == NATIVE_NAME:
                native_name_visible = True
                break
            time.sleep(0.25)
        if not native_name_visible:
            raise StudyFailure("native rename was not visible")

        time.sleep(1.0)
        screen_before_set = capture(runtime_socket, RUNTIME_SESSION)
        helpers_closed = helpers_closed and set_name(
            provider_environment,
            thread_id,
            APP_SERVER_NAME,
        )
        renamed_thread, closed = read_thread(
            provider_environment,
            thread_id,
            include_turns=False,
        )
        helpers_closed = helpers_closed and closed
        screen_after_set = capture(runtime_socket, RUNTIME_SESSION)

        current_facts_output = private_tmux(
            runtime_socket,
            "display-message",
            "-p",
            "-t",
            f"{RUNTIME_SESSION}:0.0",
            "#{pane_pid}\t#{pane_id}\t#{pane_current_path}",
        ).stdout.rstrip("\n")
        current_facts_values = current_facts_output.split("\t")
        if len(current_facts_values) != 3:
            raise StudyFailure("native TUI facts changed shape")
        current_facts = (
            current_facts_values[0],
            current_facts_values[1],
            current_facts_values[2],
            process_birth(provider_pid),
        )

        fallback_matrix = (
            resolve_name(
                state="named",
                workstream="w1",
                native_name="Native",
            )
            == ("Native", "native")
            and resolve_name(state="known_empty", workstream="w1")
            == ("untitled · w1", "synthetic")
            and resolve_name(
                state="known_empty",
                workstream="w1",
                context="cutover",
                previous_name="Source",
            )
            == ("Source ↻ unnamed", "cutover_fallback")
            and resolve_name(
                state="known_empty",
                workstream="w2",
                context="fork",
                source_name="Source",
                source_workstream="w1",
            )
            == ("Source · fork · w2", "fork_fallback")
            and resolve_name(
                state="known_empty",
                workstream="w2",
                context="fork",
                source_workstream="w1",
            )
            == ("fork of w1 · w2", "fork_fallback")
            and resolve_name(
                state="unavailable",
                workstream="w1",
                cached_name="Cached",
            )
            == ("Cached · stale", "cached_stale")
            and resolve_name(state="unavailable", workstream="w1")
            == ("name unavailable · w1", "synthetic")
        )
        after_fallback_thread, closed = read_thread(
            provider_environment,
            thread_id,
            include_turns=False,
        )
        helpers_closed = helpers_closed and closed

        assertions = {
            "ephemeral_stdio_only": stdio_command_allowed(
                ["codex", "app-server", "--listen", "stdio://"]
            ),
            "persistent_endpoints_rejected": not stdio_command_allowed(
                ["codex", "app-server", "--listen", "unix://"]
            )
            and not stdio_command_allowed(
                ["codex", "app-server", "--listen", "ws://127.0.0.1:4500"]
            ),
            "managed_remote_runtime_rejected": not managed_runtime_allowed(
                ["codex", "--remote", "ws://127.0.0.1:4500"]
            ),
            "exact_cli_thread_identified": bool(thread_id),
            "summary_read_does_not_disturb_tui": (
                screen_before_read == screen_after_read
                and facts_before_read == provider_facts
            ),
            "native_rename_visible_to_ephemeral_reader": native_name_visible,
            "app_server_rename_persisted": (
                renamed_thread.get("name") == APP_SERVER_NAME
            ),
            "name_set_does_not_disturb_tui": screen_before_set == screen_after_set,
            "native_runtime_facts_stable": current_facts == provider_facts,
            "helpers_exit_after_each_operation": helpers_closed,
            "response_fields_filtered": set(filtered) == {"name_state", "name"}
            and "preview" not in filtered,
            "name_state_classified": filtered["name_state"] in {"named", "known_empty"},
            "name_set_has_no_compare_and_set": name_set_has_no_compare_and_set,
            "fallback_matrix_complete": fallback_matrix,
            "fallback_evaluation_does_not_write_provider_name": (
                after_fallback_thread.get("name") == APP_SERVER_NAME
            ),
            "disposable_repository_unchanged": not run(
                ["git", "-C", str(workspace), "status", "--porcelain"]
            ).stdout,
        }
        if not all(assertions.values()):
            raise StudyFailure("one or more naming assertions failed")

        send_line(presentation_socket, PRESENTATION_SESSION, "/exit")
        wait_for(
            lambda: (
                private_tmux(
                    runtime_socket,
                    "has-session",
                    "-t",
                    RUNTIME_SESSION,
                    check=False,
                ).returncode
                != 0
            )
        )
        status = "pass"
        reason = "ephemeral-metadata-and-naming-boundary-proven"
    except StudyBlocked:
        status = "blocked"
        reason = "live-provider-prerequisite-unavailable"
    except (
        OSError,
        StudyFailure,
        subprocess.SubprocessError,
        ValueError,
    ):
        status = "falsified"
        reason = "provider-contract-not-proven"
    finally:
        if presentation_started and presentation_socket is not None:
            private_tmux(presentation_socket, "kill-server", check=False)
        if runtime_started and runtime_socket is not None:
            private_tmux(runtime_socket, "kill-server", check=False)
        if root is not None:
            root_string = str(root)
            if root_string.startswith(
                "/tmp/wsnav-codex-metadata-spike."
            ) and root.parent == Path("/tmp"):
                shutil.rmtree(root, ignore_errors=True)
        time.sleep(0.5)
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
        "assertions": assertions,
        "cleanup": cleanup,
        "elapsed_seconds": int(time.monotonic() - started),
    }
    write_result(arguments.result.resolve(), result)
    return 0 if status == "pass" else 1


if __name__ == "__main__":
    raise SystemExit(main())
