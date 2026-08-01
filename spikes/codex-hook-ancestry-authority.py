#!/usr/bin/env python3
"""Isolated live study of an environment-free Codex hook authority candidate."""

from __future__ import annotations

import argparse
import contextlib
import hashlib
import json
import os
import shlex
import shutil
import subprocess
import tempfile
import time
from collections.abc import Callable, Sequence
from dataclasses import dataclass
from pathlib import Path
from typing import Any

STUDY = "codex-hook-ancestry-authority"
CONTRACT = "codex-hook-parent-pid-birth-cwd-v1"
TIMEOUT_SECONDS = 90.0
MARKER_ONE = "WSNAV_ANCESTRY_ONE"
MARKER_TWO = "WSNAV_ANCESTRY_TWO"
MARKER_CLEAR_ONE = "WSNAV_ANCESTRY_CLEAR_ONE"
MARKER_CLEAR_TWO = "WSNAV_ANCESTRY_CLEAR_TWO"
STALE_MARKER = "WSNAV_ANCESTRY_STALE"
FORGED_MARKER = "WSNAV_ANCESTRY_FORGED"


class StudyFailure(RuntimeError):
    """The provider contract contradicted the candidate authority design."""


class StudyBlocked(RuntimeError):
    """The provider contract could not be exercised safely."""


@dataclass(frozen=True)
class Runtime:
    """One spike-owned private Codex TUI."""

    name: str
    socket: Path
    session: str
    barrier: Path
    process_id: int
    process_birth: str
    workspace: Path


def run(
    arguments: Sequence[str],
    *,
    environment: dict[str, str] | None = None,
    check: bool = True,
    timeout: float = 30.0,
    input_text: str | None = None,
) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        list(arguments),
        check=check,
        capture_output=True,
        text=True,
        env=environment,
        timeout=timeout,
        input=input_text,
    )


def private_tmux(
    socket: Path,
    *arguments: str,
    config: Path | None = None,
    check: bool = True,
) -> subprocess.CompletedProcess[str]:
    environment = dict(os.environ)
    environment.pop("TMUX", None)
    command = ["tmux"]
    if config is not None:
        command.extend(["-f", str(config)])
    command.extend(["-S", str(socket), *arguments])
    return run(command, environment=environment, check=check)


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


def process_stat(process_id: int) -> tuple[int, str]:
    try:
        value = Path(f"/proc/{process_id}/stat").read_text(encoding="utf-8")
        after_name = value[value.rfind(")") + 2 :].split()
        return int(after_name[1]), after_name[19]
    except (FileNotFoundError, IndexError, OSError, ValueError) as error:
        raise StudyFailure("owned provider process identity disappeared") from error


def process_environment_has_key(process_id: int, key: str) -> bool:
    try:
        entries = Path(f"/proc/{process_id}/environ").read_bytes().split(b"\0")
    except (FileNotFoundError, OSError) as error:
        raise StudyFailure("owned provider environment disappeared") from error
    prefix = f"{key}=".encode()
    return any(entry.startswith(prefix) for entry in entries)


def wait_until(predicate: Callable[[], bool], reason: str) -> None:
    deadline = time.monotonic() + TIMEOUT_SECONDS
    while time.monotonic() < deadline:
        if predicate():
            return
        time.sleep(0.2)
    raise StudyFailure(reason)


def capture(runtime: Runtime) -> str:
    return private_tmux(
        runtime.socket,
        "capture-pane",
        "-p",
        "-t",
        f"{runtime.session}:0.0",
    ).stdout


def send_prompt(runtime: Runtime, prompt: str) -> None:
    private_tmux(
        runtime.socket,
        "send-keys",
        "-t",
        f"{runtime.session}:0.0",
        "-l",
        prompt,
    )
    # The TUI's initial render is asynchronous.  Give its composer one event
    # loop after literal input before injecting the submit key.
    time.sleep(0.3)
    private_tmux(
        runtime.socket,
        "send-keys",
        "-t",
        f"{runtime.session}:0.0",
        "C-m",
    )


def wait_for_text(runtime: Runtime, expected: str, reason: str) -> None:
    wait_until(lambda: expected in capture(runtime), reason)


def wait_for_ready(runtime: Runtime) -> None:
    """Wait until the native TUI has rendered its initial idle surface."""
    wait_until(
        lambda: "OpenAI Codex" in capture(runtime) and "model:" in capture(runtime),
        "native Codex TUI did not become ready",
    )


def stop_runtime(runtime: Runtime) -> None:
    with contextlib.suppress(subprocess.CalledProcessError):
        send_prompt(runtime, "/exit")
    deadline = time.monotonic() + 10.0
    while time.monotonic() < deadline:
        result = private_tmux(
            runtime.socket,
            "has-session",
            "-t",
            runtime.session,
            check=False,
        )
        if result.returncode != 0:
            return
        time.sleep(0.1)
    private_tmux(runtime.socket, "kill-server", check=False)


def write_private(path: Path, content: str, mode: int = 0o600) -> None:
    path.write_text(content, encoding="utf-8")
    path.chmod(mode)


def append_private_line(path: Path, line: str) -> None:
    with path.open("a", encoding="utf-8") as stream:
        stream.write(line)
    path.chmod(0o600)


def hook_command(script: Path, root: Path) -> str:
    return " ".join(
        shlex.quote(part)
        for part in (
            str(Path(os.sys.executable)),
            str(script),
            "--hook-root",
            str(root),
        )
    )


def hooks_json(command: str) -> str:
    command_hook = {"type": "command", "command": command, "timeout": 3}
    value = {
        "description": "Managed by the isolated WSNav ancestry-authority spike.",
        "hooks": {
            "SessionStart": [
                {
                    "matcher": "startup|resume|clear|compact",
                    "hooks": [command_hook],
                }
            ],
            "UserPromptSubmit": [{"hooks": [command_hook]}],
            "Stop": [{"hooks": [command_hook]}],
            "SessionEnd": [{"matcher": "other", "hooks": [command_hook]}],
        },
    }
    return json.dumps(value, indent=2) + "\n"


def start_runtime(
    root: Path,
    name: str,
    codex_home: Path,
    workspace: Path,
    config: Path,
) -> Runtime:
    socket = root / f"{name}.sock"
    barrier = root / f"{name}.release"
    process_file = root / f"{name}.pid"
    session = f"wsnav-ancestry-{name}"
    shell = " ".join(
        [
            "umask 077;",
            f"printf '%s\\n' \"$$\" > {shlex.quote(str(process_file))};",
            f"while [ ! -f {shlex.quote(str(barrier))} ]; do sleep 0.02; done;",
            "exec env",
            f"CODEX_HOME={shlex.quote(str(codex_home))}",
            "WSNAV_SPIKE_SENTINEL=must-not-reach-hooks",
            "COLORTERM=truecolor",
            "codex",
            "-s",
            "workspace-write",
            "-a",
            "never",
            "-C",
            shlex.quote(str(workspace)),
        ]
    )
    private_tmux(
        socket,
        "new-session",
        "-d",
        "-s",
        session,
        "-n",
        "provider",
        "-c",
        str(workspace),
        "sh",
        "-c",
        shell,
        config=config,
    )

    def process_ready() -> bool:
        return (
            process_file.is_file()
            and process_file.read_text(encoding="utf-8").strip().isdigit()
        )

    wait_until(process_ready, "private provider wrapper did not start")
    process_id = int(process_file.read_text(encoding="utf-8").strip())
    _, birth = process_stat(process_id)
    return Runtime(name, socket, session, barrier, process_id, birth, workspace)


def release(runtime: Runtime) -> None:
    runtime.barrier.touch(mode=0o600, exist_ok=False)


def write_records(root: Path, runtimes: Sequence[Runtime]) -> None:
    records = "".join(
        f"{runtime.process_id}\t{runtime.process_birth}\t{runtime.workspace}\n"
        for runtime in runtimes
    )
    write_private(root / "runtime-records", records)


def events(root: Path) -> list[dict[str, Any]]:
    path = root / "events.jsonl"
    if not path.exists():
        return []
    values = []
    for line in path.read_text(encoding="utf-8").splitlines():
        value = json.loads(line)
        if isinstance(value, dict):
            values.append(value)
    return values


def event_count(
    root: Path,
    event: str,
    accepted: bool,
    *,
    source: str | None = None,
    session_changed: bool | None = None,
) -> int:
    return sum(
        value.get("event") == event
        and value.get("accepted") is accepted
        and (source is None or value.get("source") == source)
        and (session_changed is None or value.get("session_changed") is session_changed)
        for value in events(root)
    )


def wait_for_event_count(
    root: Path,
    event: str,
    accepted: bool,
    expected: int,
    *,
    source: str | None = None,
    session_changed: bool | None = None,
) -> None:
    wait_until(
        lambda: (
            event_count(
                root,
                event,
                accepted,
                source=source,
                session_changed=session_changed,
            )
            >= expected
        ),
        f"{event} did not reach expected authority result",
    )


def wait_briefly(predicate: Callable[[], bool], seconds: float = 4.0) -> bool:
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        if predicate():
            return True
        time.sleep(0.2)
    return predicate()


def invoke_forged_hook(root: Path, script: Path, workspace: Path) -> None:
    payload = json.dumps(
        {
            "hook_event_name": "Stop",
            "session_id": "spike-forged-session",
            "turn_id": "spike-forged-turn",
            "cwd": str(workspace),
        }
    )
    run(
        [str(Path(os.sys.executable)), str(script), "--hook-root", str(root)],
        input_text=payload,
    )


def agent_forgery_prompt(script: Path, root: Path, workspace: Path) -> str:
    payload = json.dumps(
        {
            "hook_event_name": "Stop",
            "session_id": "spike-agent-forged-session",
            "turn_id": "spike-agent-forged-turn",
            "cwd": str(workspace),
        },
        separators=(",", ":"),
    )
    command = f"printf %s {shlex.quote(payload)} | {hook_command(script, root)}"
    return (
        "Run exactly this harmless command, then reply with the exact token "
        f"{FORGED_MARKER} and nothing else. Do not inspect files or make other changes.\n"
        f"{command}"
    )


def clear_and_complete_turn(
    root: Path,
    runtime: Runtime,
    marker: str,
    expected_clear_count: int,
) -> None:
    """Exercise one native clear and the destination thread's first turn."""
    clear_before = event_count(
        root,
        "SessionStart",
        True,
        source="clear",
        session_changed=True,
    )
    stops_before = event_count(root, "Stop", True)
    send_prompt(runtime, "/clear")
    # Codex may create the destination immediately or lazily on the next
    # normal prompt.  In either case WSNav must see exactly one clear binding
    # before that destination turn settles.
    wait_briefly(
        lambda: (
            event_count(
                root,
                "SessionStart",
                True,
                source="clear",
                session_changed=True,
            )
            >= clear_before + 1
        )
    )
    send_prompt(
        runtime,
        f"Reply with the exact token {marker} and nothing else. Do not use tools.",
    )
    wait_for_event_count(
        root,
        "SessionStart",
        True,
        expected_clear_count,
        source="clear",
        session_changed=True,
    )
    wait_for_event_count(root, "Stop", True, stops_before + 1)


def hook(root: Path) -> int:
    """Drain one native hook payload and accept only an exact parent Runtime."""
    payload = os.read(0, 8 * 1024 * 1024)
    while True:
        chunk = os.read(0, 64 * 1024)
        if not chunk:
            break
        if len(payload) < 8 * 1024 * 1024:
            payload += chunk[: 8 * 1024 * 1024 - len(payload)]
    # This temporary, metadata-only breadcrumb distinguishes a hook that never
    # launched from one that launched but failed authority.  It is deliberately
    # after the complete drain, and unavailable state is a silent no-op.
    try:
        append_private_line(root / "invocations.jsonl", '{"invoked":true}\n')
    except OSError:
        return 0
    try:
        value = json.loads(payload)
        if not isinstance(value, dict):
            raise TypeError
    except (UnicodeDecodeError, TypeError, json.JSONDecodeError):
        return 0

    event = value.get("hook_event_name")
    session_id = value.get("session_id")
    payload_cwd = value.get("cwd")
    source = value.get("source")
    allowed_event = event in {"SessionStart", "UserPromptSubmit", "Stop", "SessionEnd"}
    valid_shape = (
        allowed_event
        and isinstance(session_id, str)
        and bool(session_id)
        and isinstance(payload_cwd, str)
        and bool(payload_cwd)
    )
    record_matches = 0
    provider_depth: int | None = None
    candidate = os.getppid()
    # Current Codex launches a native command hook directly.  Do not permit a
    # shell wrapper here: an agent tool shell would otherwise be one hop below
    # the provider and could forge a lifecycle payload.
    for depth in range(1):
        try:
            parent, birth = process_stat(candidate)
            cwd = os.readlink(f"/proc/{candidate}/cwd")
            records = (
                (root / "runtime-records").read_text(encoding="utf-8").splitlines()
            )
        except (FileNotFoundError, OSError, StudyFailure):
            break
        matches = sum(
            record == f"{candidate}\t{birth}\t{payload_cwd}" and cwd == payload_cwd
            for record in records
        )
        record_matches += matches
        if matches == 1:
            provider_depth = depth
        candidate = parent

    accepted = valid_shape and record_matches == 1 and provider_depth is not None
    reason = "accepted" if accepted else "forged-process"
    source_kind = (
        source if source in {"startup", "resume", "clear", "compact"} else "other"
    )
    session_changed = False
    if accepted and event == "SessionStart":
        # The temporary log retains only a one-way digest to express whether
        # Codex moved this Runtime to a distinct native thread.  It is deleted
        # with the disposable root and never reaches a fixture.
        fingerprint = hashlib.sha256(session_id.encode()).hexdigest()
        fingerprints = root / "session-fingerprints"
        previous = ""
        try:
            provider_identity = f"{os.getppid()}:{process_stat(os.getppid())[1]}"
            if fingerprints.exists():
                for line in fingerprints.read_text(encoding="utf-8").splitlines():
                    birth, _, seen = line.partition("\t")
                    if birth == provider_identity:
                        previous = seen
        except (OSError, StudyFailure):
            return 0
        session_changed = bool(previous) and previous != fingerprint
        try:
            append_private_line(
                fingerprints,
                f"{provider_identity}\t{fingerprint}\n",
            )
        except OSError:
            return 0
    entry = {
        "event": event if isinstance(event, str) else "unknown",
        "accepted": accepted,
        "reason": reason,
        "record_matches": record_matches,
        "provider_depth": provider_depth,
        "source": source_kind,
        "session_changed": session_changed,
    }
    event_log = root / "events.jsonl"
    try:
        append_private_line(event_log, json.dumps(entry, separators=(",", ":")) + "\n")
    except OSError:
        return 0
    return 0


def write_result(path: Path | None, result: dict[str, Any]) -> None:
    output = json.dumps(result, indent=2, sort_keys=True) + "\n"
    if path is None:
        print(output, end="")
        return
    write_private(path, output)


def study() -> tuple[str, str, dict[str, bool]]:
    before: str | None = None
    root: Path | None = None
    runtimes: list[Runtime] = []
    assertions = {
        "trusted_hook_source_reused": False,
        "codex_environment_sanitized": False,
        "two_private_runtimes_exactly_matched": False,
        "repeated_native_clear_rebinding": False,
        "external_shell_forgery_rejected": False,
        "agent_shell_forgery_rejected": False,
        "stale_runtime_record_rejected": False,
        "ordinary_tmux_unchanged": False,
        "cleanup_complete": False,
    }
    try:
        for command in ("codex", "git", "tmux"):
            if shutil.which(command) is None:
                raise StudyBlocked(f"required command unavailable: {command}")
        auth = Path.home() / ".codex" / "auth.json"
        if not auth.is_file():
            raise StudyBlocked("no readable Codex auth cache")

        before = ordinary_tmux_fingerprint()
        root = Path(tempfile.mkdtemp(prefix="wsnav-codex-ancestry-spike."))
        root.chmod(0o700)
        codex_home = root / "codex-home"
        workspace_one = root / "workspace-one"
        workspace_two = root / "workspace-two"
        codex_home.mkdir(mode=0o700)
        workspace_one.mkdir(mode=0o700)
        workspace_two.mkdir(mode=0o700)
        shutil.copyfile(auth, codex_home / "auth.json")
        (codex_home / "auth.json").chmod(0o600)
        for workspace in (workspace_one, workspace_two):
            run(["git", "-C", str(workspace), "init", "-q"])

        script = Path(__file__).resolve()
        command = hook_command(script, root)
        write_private(
            codex_home / "config.toml",
            "[features]\nhooks = true\n"
            f'[projects.{json.dumps(str(workspace_one))}]\ntrust_level = "trusted"\n'
            f'[projects.{json.dumps(str(workspace_two))}]\ntrust_level = "trusted"\n',
        )
        write_private(codex_home / "hooks.json", hooks_json(command))
        config = root / "tmux.conf"
        write_private(
            config,
            "set -g status off\nset -g mouse on\nset -g default-terminal tmux-256color\n",
        )
        write_private(root / "events.jsonl", "")

        review = start_runtime(root, "review", codex_home, workspace_one, config)
        runtimes.append(review)
        write_records(root, [review])
        release(review)
        wait_for_text(
            review, "Trust all and continue", "native hook review was not visible"
        )
        private_tmux(review.socket, "send-keys", "-t", f"{review.session}:0.0", "Down")
        private_tmux(review.socket, "send-keys", "-t", f"{review.session}:0.0", "Enter")
        wait_until(
            lambda: "Trust all and continue" not in capture(review),
            "native hook trust was not accepted",
        )
        stop_runtime(review)
        runtimes.remove(review)
        write_private(root / "events.jsonl", "")

        first = start_runtime(root, "one", codex_home, workspace_one, config)
        second = start_runtime(root, "two", codex_home, workspace_two, config)
        runtimes.extend([first, second])
        write_records(root, [first, second])
        release(first)
        release(second)
        assertions["trusted_hook_source_reused"] = True
        assertions["codex_environment_sanitized"] = not process_environment_has_key(
            first.process_id, "WSNAV_SPIKE_SENTINEL"
        ) and not process_environment_has_key(second.process_id, "WSNAV_SPIKE_SENTINEL")
        wait_for_ready(first)
        wait_for_ready(second)

        session_start_before = event_count(root, "SessionStart", True)
        prompt_events_before = event_count(root, "UserPromptSubmit", True)
        stop_events_before = event_count(root, "Stop", True)
        send_prompt(
            first,
            f"Reply with the exact token {MARKER_ONE} and nothing else. Do not use tools.",
        )
        send_prompt(
            second,
            f"Reply with the exact token {MARKER_TWO} and nothing else. Do not use tools.",
        )
        wait_for_event_count(root, "SessionStart", True, session_start_before + 2)
        wait_for_event_count(root, "UserPromptSubmit", True, prompt_events_before + 2)
        wait_for_event_count(root, "Stop", True, stop_events_before + 2)
        assertions["two_private_runtimes_exactly_matched"] = True

        clear_and_complete_turn(root, first, MARKER_CLEAR_ONE, 1)
        clear_and_complete_turn(root, first, MARKER_CLEAR_TWO, 2)
        _, process_birth_after_clear = process_stat(first.process_id)
        if process_birth_after_clear != first.process_birth:
            raise StudyFailure("native clear restarted the managed Codex process")
        assertions["repeated_native_clear_rebinding"] = True

        rejected_before = event_count(root, "Stop", False)
        invoke_forged_hook(root, script, workspace_one)
        wait_for_event_count(root, "Stop", False, rejected_before + 1)
        assertions["external_shell_forgery_rejected"] = True

        write_records(root, [second])
        rejected_before = event_count(root, "UserPromptSubmit", False)
        send_prompt(
            first,
            f"Reply with the exact token {STALE_MARKER} and nothing else. Do not use tools.",
        )
        wait_for_text(first, STALE_MARKER, "stale-record turn did not complete")
        wait_for_event_count(root, "UserPromptSubmit", False, rejected_before + 1)
        assertions["stale_runtime_record_rejected"] = True
        write_records(root, [first, second])

        rejected_before = event_count(root, "Stop", False)
        send_prompt(first, agent_forgery_prompt(script, root, workspace_one))
        wait_for_text(first, FORGED_MARKER, "agent-shell forgery turn did not complete")
        wait_for_event_count(root, "Stop", False, rejected_before + 1)
        assertions["agent_shell_forgery_rejected"] = True

        return "pass", "ancestry-record-authority-proven", assertions
    except StudyBlocked as error:
        return "blocked", str(error), assertions
    except StudyFailure as error:
        return "falsified", str(error), assertions
    except subprocess.CalledProcessError:
        return "blocked", "harness-command-failed", assertions
    except subprocess.TimeoutExpired:
        return "blocked", "harness-timed-out", assertions
    except Exception as error:  # noqa: BLE001 - always emit a sanitized fixture.
        return "blocked", f"harness-error:{type(error).__name__}", assertions
    finally:
        for runtime in reversed(runtimes):
            with contextlib.suppress(
                subprocess.CalledProcessError, subprocess.TimeoutExpired
            ):
                stop_runtime(runtime)
        if root is not None:
            with contextlib.suppress(FileNotFoundError):
                shutil.rmtree(root)
            assertions["cleanup_complete"] = not root.exists()
        if before is not None:
            assertions["ordinary_tmux_unchanged"] = (
                ordinary_tmux_fingerprint() == before
            )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--result", type=Path)
    parser.add_argument("--hook-root", type=Path, help=argparse.SUPPRESS)
    arguments = parser.parse_args()
    if arguments.hook_root is not None:
        return hook(arguments.hook_root)

    started = time.monotonic()
    status, reason, assertions = study()
    result = {
        "study": STUDY,
        "provider": {
            "id": "codex",
            "version": run(["codex", "--version"], check=False).stdout.strip(),
            "contract_fingerprint": CONTRACT,
        },
        "status": status,
        "reason": reason,
        "assertions": assertions,
        "privacy_audit": {
            "provider_or_workstream_identifiers_committed": False,
            "prompt_or_result_content_committed": False,
            "terminal_data_committed": False,
            "paths_or_process_ids_committed": False,
            "credentials_or_raw_payloads_committed": False,
        },
        "elapsed_seconds": round(time.monotonic() - started, 1),
    }
    write_result(arguments.result, result)
    return {"pass": 0, "falsified": 1, "blocked": 2}[status]


if __name__ == "__main__":
    raise SystemExit(main())
