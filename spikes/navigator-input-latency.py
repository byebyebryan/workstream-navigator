#!/usr/bin/env python3
"""Measure synthetic input delivery separately from nested-tmux visual echo.

This study creates no provider or Workstream state.  A raw-mode endpoint is
attached through a private runtime tmux server and a private presentation
server.  The presentation has a provider pane attached to the runtime and a
narrow synthetic navigator pane.  The client sends bounded timestamp tokens;
the endpoint returns only a monotonic receive timestamp, allowing the harness
to distinguish input delivery from the time until the acknowledgement becomes
visible at the client PTY.
"""

from __future__ import annotations

import argparse
import contextlib
import fcntl
import hashlib
import json
import math
import os
import pty
import re
import selectors
import shlex
import shutil
import signal
import struct
import subprocess
import tempfile
import termios
import time
from collections.abc import Sequence
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Final

STUDY: Final = "navigator-input-latency"
CONTRACT: Final = "nested-tmux-input-echo-ab-v1"
ROOT_PREFIX: Final = "wsnav-navigator-input-latency."
SAMPLE_COUNT: Final = 90
SAMPLE_INTERVAL_SECONDS: Final = 0.04
ACK_TIMEOUT_SECONDS: Final = 0.8
CLIENT_COLUMNS: Final = 120
CLIENT_ROWS: Final = 32
NAVIGATOR_WIDTH: Final = 28
MAX_CLIENT_BUFFER_BYTES: Final = 512 * 1024

ACK_PATTERN: Final = re.compile(rb"ACK:(\d+):(\d+)")
ANSI_PATTERN: Final = re.compile(
    rb"\x1b(?:\[[0-?]*[ -/]*[@-~]|\][^\x07]*(?:\x07|\x1b\\))"
)


class StudyFailure(RuntimeError):
    """The synthetic topology or measurement contradicted the study."""


class StudyBlocked(RuntimeError):
    """The local disposable measurement could not be started safely."""


ENDPOINT_SOURCE: Final = r'''#!/usr/bin/env python3
"""Synthetic raw-mode endpoint used only inside a disposable tmux pane."""

import os
import sys
import termios
import time
import tty


def main() -> int:
    fd = sys.stdin.fileno()
    original = termios.tcgetattr(fd)
    tty.setraw(fd)
    buffer = bytearray()
    prefix = b"WSNAV_LATENCY_"
    token_length = len(prefix) + 3
    try:
        while True:
            chunk = os.read(fd, 1)
            if not chunk:
                return 0
            buffer.extend(chunk)
            if len(buffer) == token_length:
                if buffer.startswith(prefix):
                    try:
                        sequence = int(buffer[len(prefix) :])
                    except ValueError:
                        sequence = -1
                    if sequence >= 0:
                        received = time.monotonic_ns()
                        os.write(1, f"\x1b[1GACK:{sequence}:{received}\x1b[K".encode())
                buffer.clear()
    finally:
        termios.tcsetattr(fd, termios.TCSANOW, original)


if __name__ == "__main__":
    raise SystemExit(main())
'''


NAVIGATOR_SOURCE: Final = r'''#!/usr/bin/env python3
"""Synthetic navigator pane: static once or a full redraw every 100 ms."""

import os
import sys
import time


FRAME = (
    "\x1b[?25l\x1b[2J\x1b[H"
    + "".join(f"\x1b[{row};1H\x1b[33m●\x1b[39m synthetic navigator {row:02d}" for row in range(1, 25))
    + "\x1b[?25h"
).encode()


def main() -> int:
    mode = sys.argv[1]
    while True:
        os.write(1, FRAME)
        if mode == "static":
            time.sleep(60)
        else:
            time.sleep(0.1)


if __name__ == "__main__":
    raise SystemExit(main())
'''


def run(
    arguments: Sequence[str],
    *,
    environment: dict[str, str] | None = None,
    timeout: float = 5.0,
    check: bool = False,
) -> subprocess.CompletedProcess[str]:
    try:
        result = subprocess.run(
            list(arguments),
            capture_output=True,
            check=False,
            env=None if environment is None else dict(environment),
            text=True,
            timeout=timeout,
        )
    except subprocess.TimeoutExpired as error:
        raise StudyFailure("subprocess-timeout") from error
    except OSError as error:
        raise StudyBlocked("required-subprocess-unavailable") from error
    if check and result.returncode != 0:
        raise StudyFailure("private-tmux-command-failed")
    return result


def private_tmux(
    socket: Path,
    configuration: Path,
    *arguments: str,
    check: bool = True,
) -> subprocess.CompletedProcess[str]:
    environment = dict(os.environ)
    environment.pop("TMUX", None)
    return run(
        ["tmux", "-f", str(configuration), "-S", str(socket), *arguments],
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
    return hashlib.sha256(result.stdout.encode("utf-8")).hexdigest()


def tmux_version() -> str:
    result = run(["tmux", "-V"], check=False)
    if result.returncode != 0:
        raise StudyBlocked("tmux-version-unavailable")
    fields = result.stdout.strip().split()
    if len(fields) != 2 or fields[0] != "tmux" or not fields[1]:
        raise StudyFailure("tmux-version-malformed")
    return fields[1]


def write_private_file(path: Path, content: str, mode: int) -> None:
    path.write_text(content, encoding="utf-8")
    path.chmod(mode)


def write_tmux_config(path: Path) -> None:
    write_private_file(
        path,
        'set -g default-terminal "tmux-256color"\n'
        "set -g status off\n"
        "set -g mouse off\n"
        "set -g escape-time 0\n"
        "set -g history-limit 100\n",
        0o600,
    )


def tmux_output(
    socket: Path,
    configuration: Path,
    *arguments: str,
) -> str:
    result = private_tmux(socket, configuration, *arguments, check=False)
    if result.returncode != 0:
        return ""
    return result.stdout


def kill_private_tmux(socket: Path, configuration: Path) -> bool:
    result = private_tmux(socket, configuration, "kill-server", check=False)
    with contextlib.suppress(FileNotFoundError):
        socket.unlink()
    return result.returncode == 0 or not socket.exists()


def set_pty_size(fd: int) -> None:
    dimensions = struct.pack("HHHH", CLIENT_ROWS, CLIENT_COLUMNS, 0, 0)
    fcntl.ioctl(fd, termios.TIOCSWINSZ, dimensions)


def percentile(values: Sequence[float], fraction: float) -> float:
    if not values:
        raise StudyFailure("no-measurements")
    ordered = sorted(values)
    index = max(0, math.ceil(len(ordered) * fraction) - 1)
    return ordered[index]


def aggregate(values: Sequence[float]) -> dict[str, float]:
    if not values:
        raise StudyFailure("no-measurements")
    ordered = sorted(values)
    return {
        "median_ms": round(float(ordered[len(ordered) // 2]), 3),
        "p95_ms": round(percentile(ordered, 0.95), 3),
        "max_ms": round(float(ordered[-1]), 3),
    }


def terminate_process(process: subprocess.Popen[bytes] | None) -> bool:
    if process is None:
        return True
    if process.poll() is None:
        with contextlib.suppress(ProcessLookupError):
            os.killpg(process.pid, signal.SIGTERM)
        try:
            process.wait(timeout=0.5)
        except subprocess.TimeoutExpired:
            with contextlib.suppress(ProcessLookupError):
                os.killpg(process.pid, signal.SIGKILL)
            with contextlib.suppress(subprocess.TimeoutExpired):
                process.wait(timeout=0.5)
    return process.poll() is not None


@dataclass
class LatencyCase:
    mode: str
    root: Path
    endpoint_source: Path
    navigator_source: Path
    client_process: subprocess.Popen[bytes] | None = None
    client_master: int | None = None
    client_ack_seen: bool = False
    client_ack_for_sequence: bool = False
    cleaned: bool = False

    @property
    def runtime_socket(self) -> Path:
        return self.root / "runtime.sock"

    @property
    def presentation_socket(self) -> Path:
        return self.root / "presentation.sock"

    @property
    def runtime_config(self) -> Path:
        return self.root / "runtime.conf"

    @property
    def presentation_config(self) -> Path:
        return self.root / "presentation.conf"

    @property
    def runtime_session(self) -> str:
        return "runtime"

    @property
    def presentation_session(self) -> str:
        return "presentation"

    def setup(self) -> None:
        write_tmux_config(self.runtime_config)
        write_tmux_config(self.presentation_config)
        endpoint_command = shlex.join(["python3", str(self.endpoint_source)])
        private_tmux(
            self.runtime_socket,
            self.runtime_config,
            "new-session",
            "-d",
            "-s",
            self.runtime_session,
            "-x",
            str(CLIENT_COLUMNS),
            "-y",
            str(CLIENT_ROWS),
            "-n",
            "runtime",
            endpoint_command,
        )
        provider_command = shlex.join(
            [
                "env",
                "-u",
                "TMUX",
                "tmux",
                "-f",
                str(self.runtime_config),
                "-S",
                str(self.runtime_socket),
                "attach-session",
                "-t",
                self.runtime_session,
            ]
        )
        navigator_command = shlex.join(
            ["python3", str(self.navigator_source), self.mode]
        )
        private_tmux(
            self.presentation_socket,
            self.presentation_config,
            "new-session",
            "-d",
            "-s",
            self.presentation_session,
            "-x",
            str(CLIENT_COLUMNS),
            "-y",
            str(CLIENT_ROWS),
            "-n",
            "main",
            navigator_command,
        )
        private_tmux(
            self.presentation_socket,
            self.presentation_config,
            "split-window",
            "-h",
            "-d",
            "-t",
            f"{self.presentation_session}:0",
            "-l",
            str(CLIENT_COLUMNS - NAVIGATOR_WIDTH - 1),
            provider_command,
        )
        private_tmux(
            self.presentation_socket,
            self.presentation_config,
            "select-pane",
            "-t",
            f"{self.presentation_session}:0.1",
        )
        self._assert_layout()
        time.sleep(0.25)

    def _assert_layout(self) -> None:
        runtime_sessions = tmux_output(
            self.runtime_socket,
            self.runtime_config,
            "list-sessions",
            "-F",
            "#{session_name}",
        ).splitlines()
        if runtime_sessions != [self.runtime_session]:
            raise StudyFailure("runtime-session-layout-invalid")
        runtime_panes = tmux_output(
            self.runtime_socket,
            self.runtime_config,
            "list-panes",
            "-t",
            f"{self.runtime_session}:0",
            "-F",
            "#{pane_dead}",
        ).splitlines()
        if runtime_panes != ["0"]:
            raise StudyFailure("runtime-pane-layout-invalid")

        presentation_sessions = tmux_output(
            self.presentation_socket,
            self.presentation_config,
            "list-sessions",
            "-F",
            "#{session_name}",
        ).splitlines()
        if presentation_sessions != [self.presentation_session]:
            raise StudyFailure("presentation-session-layout-invalid")
        panes = tmux_output(
            self.presentation_socket,
            self.presentation_config,
            "list-panes",
            "-t",
            f"{self.presentation_session}:0",
            "-F",
            "#{pane_index}:#{pane_active}:#{pane_dead}:#{pane_width}",
        ).splitlines()
        if (
            len(panes) != 2
            or not panes[0].startswith("0:0:0:")
            or not panes[1].startswith("1:1:0:")
        ):
            raise StudyFailure("presentation-pane-layout-invalid")
        widths = []
        for pane in panes:
            try:
                widths.append(int(pane.rsplit(":", 1)[1]))
            except (ValueError, IndexError) as error:
                raise StudyFailure("presentation-pane-width-invalid") from error
        if min(widths) > NAVIGATOR_WIDTH + 2 or max(widths) < CLIENT_COLUMNS - 40:
            raise StudyFailure("presentation-pane-width-invalid")

    def _attach_client(self) -> None:
        master, slave = pty.openpty()
        try:
            set_pty_size(master)
            environment = dict(os.environ)
            environment.pop("TMUX", None)
            environment["TERM"] = "xterm-256color"
            environment["COLORTERM"] = "truecolor"
            self.client_process = subprocess.Popen(
                [
                    "tmux",
                    "-f",
                    str(self.presentation_config),
                    "-S",
                    str(self.presentation_socket),
                    "attach-session",
                    "-t",
                    self.presentation_session,
                ],
                env=environment,
                stdin=slave,
                stdout=slave,
                stderr=slave,
                close_fds=True,
                start_new_session=True,
            )
        finally:
            os.close(slave)
        self.client_master = master
        time.sleep(0.25)
        self._drain_client()

    def _drain_client(self) -> None:
        if self.client_master is None:
            return
        selector = selectors.DefaultSelector()
        try:
            selector.register(self.client_master, selectors.EVENT_READ)
            while True:
                events = selector.select(0)
                if not events:
                    return
                try:
                    if not os.read(self.client_master, 65536):
                        return
                except OSError:
                    return
        finally:
            selector.close()

    def _receive_ack(self, sequence: int) -> tuple[int, int]:
        if self.client_master is None or self.client_process is None:
            raise StudyFailure("presentation-client-unavailable")
        self.client_ack_seen = False
        self.client_ack_for_sequence = False
        selector = selectors.DefaultSelector()
        buffer = bytearray()
        deadline = time.monotonic() + ACK_TIMEOUT_SECONDS
        try:
            selector.register(self.client_master, selectors.EVENT_READ)
            while time.monotonic() < deadline:
                if self.client_process.poll() is not None:
                    raise StudyFailure("presentation-client-exited")
                events = selector.select(max(0.0, deadline - time.monotonic()))
                if not events:
                    continue
                try:
                    chunk = os.read(self.client_master, 65536)
                except OSError as error:
                    raise StudyFailure("presentation-client-read-failed") from error
                if not chunk:
                    raise StudyFailure("presentation-client-closed")
                buffer.extend(chunk)
                if len(buffer) > MAX_CLIENT_BUFFER_BYTES:
                    raise StudyFailure("presentation-client-output-unbounded")
                visible = ANSI_PATTERN.sub(b"", buffer)
                matches = list(ACK_PATTERN.finditer(visible))
                self.client_ack_seen |= bool(matches)
                self.client_ack_for_sequence |= any(
                    int(match.group(1)) == sequence for match in matches
                )
                for match in matches:
                    if int(match.group(1)) == sequence:
                        return int(match.group(2)), time.monotonic_ns()
                if len(buffer) > 4096:
                    del buffer[:-512]
        finally:
            selector.close()
        raise StudyFailure("acknowledgement-timeout")

    def _timeout_evidence(self) -> str:
        runtime_capture = tmux_output(
            self.runtime_socket,
            self.runtime_config,
            "capture-pane",
            "-p",
            "-t",
            f"{self.runtime_session}:0.0",
        )
        presentation_capture = tmux_output(
            self.presentation_socket,
            self.presentation_config,
            "capture-pane",
            "-p",
            "-t",
            f"{self.presentation_session}:0.1",
        )
        endpoint_alive = (
            tmux_output(
                self.runtime_socket,
                self.runtime_config,
                "list-panes",
                "-t",
                f"{self.runtime_session}:0",
                "-F",
                "#{pane_dead}",
            ).strip()
            == "0"
        )
        runtime_ack = "ACK:" in runtime_capture
        presentation_ack = "ACK:" in presentation_capture
        runtime_ack_count = runtime_capture.count("ACK:")
        presentation_ack_count = presentation_capture.count("ACK:")
        return (
            "acknowledgement-timeout"
            f":runtime_ack={runtime_ack}"
            f":presentation_ack={presentation_ack}"
            f":runtime_ack_count={runtime_ack_count}"
            f":presentation_ack_count={presentation_ack_count}"
            f":client_ack={self.client_ack_seen}"
            f":client_sequence={self.client_ack_for_sequence}"
            f":endpoint_alive={endpoint_alive}"
        )

    def measure(self) -> dict[str, Any]:
        self._attach_client()
        delivery: list[float] = []
        echo: list[float] = []
        if self.client_master is None:
            raise StudyFailure("presentation-client-unavailable")
        for sequence in range(SAMPLE_COUNT):
            token = f"WSNAV_LATENCY_{sequence:03d}".encode()
            send_ns = time.monotonic_ns()
            try:
                offset = 0
                while offset < len(token):
                    offset += os.write(self.client_master, token[offset:])
            except OSError as error:
                raise StudyFailure("presentation-client-write-failed") from error
            try:
                receive_ns, observed_ns = self._receive_ack(sequence)
            except StudyFailure as error:
                if str(error) == "acknowledgement-timeout":
                    raise StudyFailure(self._timeout_evidence()) from error
                raise
            delivery.append(max(0, receive_ns - send_ns) / 1_000_000)
            echo.append(max(0, observed_ns - send_ns) / 1_000_000)
            if sequence + 1 < SAMPLE_COUNT:
                time.sleep(SAMPLE_INTERVAL_SECONDS)
        return {
            "sample_count": len(delivery),
            "input_delivery_ms": aggregate(delivery),
            "echo_ms": aggregate(echo),
        }

    def cleanup(self) -> bool:
        client_clean = terminate_process(self.client_process)
        if self.client_master is not None:
            with contextlib.suppress(OSError):
                os.close(self.client_master)
            self.client_master = None
        runtime_clean = kill_private_tmux(self.runtime_socket, self.runtime_config)
        presentation_clean = kill_private_tmux(
            self.presentation_socket, self.presentation_config
        )
        self.cleaned = client_clean and runtime_clean and presentation_clean
        return self.cleaned


def raise_signal() -> None:
    """Convert an interrupt into the normal bounded cleanup path."""

    raise StudyFailure("interrupted")


def write_result(path: Path, value: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    temporary = path.with_name(f".{path.name}.tmp")
    temporary.write_text(json.dumps(value, indent=2) + "\n", encoding="utf-8")
    temporary.chmod(0o600)
    temporary.replace(path)
    path.chmod(0o600)


def ratio(animated: dict[str, float], static: dict[str, float]) -> float:
    denominator = static["p95_ms"]
    if denominator == 0:
        return 0.0 if animated["p95_ms"] == 0 else float("inf")
    return round(animated["p95_ms"] / denominator, 3)


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--result", type=Path)
    arguments = parser.parse_args(argv)
    started = time.monotonic()
    root: Path | None = None
    cases: list[LatencyCase] = []
    ordinary_before = "unknown"
    ordinary_after = "unknown"
    version = "unknown"
    measurements: dict[str, dict[str, Any]] = {}
    reason: str | None = None
    status = "blocked"
    cleanup_complete = False
    try:
        for command in ("tmux", "python3"):
            if shutil.which(command) is None:
                raise StudyBlocked(f"{command}-unavailable")
        version = tmux_version()
        ordinary_before = ordinary_tmux_fingerprint()
        root = Path(tempfile.mkdtemp(prefix=ROOT_PREFIX, dir="/tmp"))
        root.chmod(0o700)
        endpoint_source = root / "endpoint.py"
        navigator_source = root / "navigator.py"
        write_private_file(endpoint_source, ENDPOINT_SOURCE, 0o700)
        write_private_file(navigator_source, NAVIGATOR_SOURCE, 0o700)
        for mode in ("static", "animated_10fps"):
            case_root = root / mode
            case_root.mkdir(mode=0o700)
            case = LatencyCase(mode, case_root, endpoint_source, navigator_source)
            cases.append(case)
            case.setup()
            measurements[mode] = case.measure()
            if not case.cleanup():
                raise StudyFailure("case-cleanup-incomplete")
        status = "pass"
    except StudyBlocked as error:
        reason = str(error)
        status = "blocked"
    except StudyFailure as error:
        reason = str(error)
        status = "falsified"
    except KeyboardInterrupt:
        reason = "interrupted"
        status = "blocked"
    except Exception:  # noqa: BLE001 - sanitize all unexpected study failures
        reason = "unexpected-error"
        status = "blocked"
    finally:
        for case in reversed(cases):
            cleanup_complete = case.cleanup() and cleanup_complete
        if cases and all(case.cleaned for case in cases):
            cleanup_complete = True
        if root is not None:
            with contextlib.suppress(OSError):
                shutil.rmtree(root)
        ordinary_after = ordinary_tmux_fingerprint()

    ordinary_unchanged = (
        ordinary_before != "unknown" and ordinary_before == ordinary_after
    )
    static = measurements.get("static")
    animated = measurements.get("animated_10fps")
    assertions = {
        "both_cases_return_every_sample": bool(
            static
            and animated
            and static["sample_count"] == SAMPLE_COUNT
            and animated["sample_count"] == SAMPLE_COUNT
        ),
        "cleanup_complete": cleanup_complete,
        "ordinary_tmux_unchanged": ordinary_unchanged,
        "static_input_delivery_p95_le_25_ms": bool(
            static and static["input_delivery_ms"]["p95_ms"] <= 25.0
        ),
        "static_echo_p95_le_50_ms": bool(
            static and static["echo_ms"]["p95_ms"] <= 50.0
        ),
    }
    if status == "pass" and not all(assertions.values()):
        status = "falsified"
        reason = "threshold-or-noninterference-assertion-failed"
    result: dict[str, Any] = {
        "study": STUDY,
        "contract_fingerprint": CONTRACT,
        "status": status,
        "environment": {"tmux_version": version},
        "sample_count": SAMPLE_COUNT,
        "static": static,
        "animated_10fps": animated,
        "animated_static_p95_ratios": (
            {
                "input_delivery": ratio(
                    animated["input_delivery_ms"], static["input_delivery_ms"]
                ),
                "echo": ratio(animated["echo_ms"], static["echo_ms"]),
            }
            if static and animated
            else None
        ),
        "assertions": assertions,
        "cleanup_complete": cleanup_complete,
        "ordinary_tmux_unchanged": ordinary_unchanged,
        "elapsed_seconds": round(time.monotonic() - started, 3),
        "limitation": "synthetic local PTY/tmux evidence; no SSH, network, or provider latency claim",
    }
    if reason is not None:
        result["reason"] = reason
    if arguments.result is not None:
        write_result(arguments.result, result)
    print(json.dumps(result, indent=2))
    return 0 if status == "pass" else 1


if __name__ == "__main__":
    signal.signal(signal.SIGINT, lambda _signum, _frame: raise_signal())
    signal.signal(signal.SIGTERM, lambda _signum, _frame: raise_signal())
    raise SystemExit(main())
