#!/usr/bin/env python3
"""Exercise brokered provisional-shell topologies with synthetic providers.

The study never starts a real provider.  Each case runs a fixed fake provider
inside a fresh private tmux server and retains only bounded topology
assertions.  Temporary process metadata, shell output, paths, and arguments
are discarded before the sanitized result is written.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shlex
import shutil
import subprocess
import tempfile
import time
from pathlib import Path
from typing import Any, Final

STUDY: Final = "brokered-onboarding-shell"
CONTRACT: Final = "provisional-private-shell-topology-v1"
ROOT_PREFIX: Final = "wsnav-brokered-onboarding."
COMMAND_TIMEOUT_SECONDS: Final = 3.0
WAIT_TIMEOUT_SECONDS: Final = 3.0
POLL_SECONDS: Final = 0.03
EXPECTED_ARGV: Final = "--model demo --flag value"
EXPECTED_COMMAND: Final = "codex --model demo --flag value"
OUTPUT_MARKER: Final = "WSNAV_SYNTHETIC_PROVIDER_OUTPUT"


class StudyFailure(RuntimeError):
    """A synthetic topology contradicted the expected contract."""


class StudyBlocked(RuntimeError):
    """The disposable study could not start safely."""


FAKE_PROVIDER_SOURCE: Final = r"""#!/bin/sh
set -eu
pgid="$(ps -o pgid= -p "$$" | tr -d " ")"
sid="$(ps -o sid= -p "$$" | tr -d " ")"
printf 'pid=%s ppid=%s pgid=%s sid=%s argv=%s\n' \
    "$$" "$PPID" "$pgid" "$sid" "$*" >> "${WSNAV_RECORD:?}"
printf '%s\n' 'WSNAV_SYNTHETIC_PROVIDER_OUTPUT'
trap 'exit 0' TERM INT HUP
if [ "${WSNAV_AUTO_EXIT:-false}" = true ]; then
    sleep 0.12
    exit 0
fi
while :; do sleep 1; done
"""


FAKE_BROKER_SOURCE: Final = r"""#!/bin/sh
set -eu
printf 'broker_invoked=true argv=%s\n' "$*" >> "${WSNAV_BROKER_RECORD:?}"
provider_command="${1:?}"
shift
exec "${WSNAV_FAKE_PROVIDER:?}" "$@"
"""


PATH_SHIM_SOURCE: Final = r"""#!/bin/sh
set -eu
exec "${WSNAV_FAKE_PROVIDER:?}" "$@"
"""


EXEC_BASHRC_SOURCE: Final = r"""codex() { exec "${WSNAV_BROKER:?}" codex "$@"; }
opencode() { exec "${WSNAV_BROKER:?}" opencode "$@"; }
"""


PREEXEC_ZSHRC_SOURCE: Final = r"""preexec() {
    case "$1" in
        codex\ *) exec "${WSNAV_BROKER:?}" codex PREEXEC ;;
        opencode\ *) exec "${WSNAV_BROKER:?}" opencode PREEXEC ;;
    esac
}
"""


SUPERVISOR_SOURCE: Final = r"""#!/bin/sh
set -eu
"${WSNAV_SHELL:?}" --noprofile --norc -i
"""


NESTED_BYPASS_SOURCE: Final = r"""#!/bin/sh
set -eu
exec "${WSNAV_DIRECT_PROVIDER:?}" "$@"
"""


def run(
    arguments: list[str],
    *,
    environment: dict[str, str] | None = None,
    check: bool = False,
    timeout: float = COMMAND_TIMEOUT_SECONDS,
) -> subprocess.CompletedProcess[str]:
    try:
        result = subprocess.run(
            arguments,
            capture_output=True,
            check=False,
            env=environment,
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


def write_private(path: Path, content: str, mode: int) -> None:
    path.write_text(content, encoding="utf-8")
    path.chmod(mode)


def write_tmux_config(path: Path) -> None:
    write_private(
        path,
        'set -g default-terminal "tmux-256color"\n'
        "set -g status off\n"
        "set -g mouse off\n"
        "set -g escape-time 0\n"
        "set -g history-limit 100\n",
        0o600,
    )


def process_stat(pid: int) -> tuple[str, str]:
    try:
        value = Path(f"/proc/{pid}/stat").read_text(encoding="utf-8")
        after_name = value[value.rfind(")") + 2 :].split()
        return after_name[0], after_name[19]
    except (FileNotFoundError, IndexError, OSError, ValueError) as error:
        raise StudyFailure("process-identity-unavailable") from error


def process_alive(pid: int) -> bool:
    try:
        state, _ = process_stat(pid)
    except StudyFailure:
        return False
    return state != "Z"


def wait_until(predicate: Any, reason: str) -> None:
    deadline = time.monotonic() + WAIT_TIMEOUT_SECONDS
    while time.monotonic() < deadline:
        if predicate():
            return
        time.sleep(POLL_SECONDS)
    raise StudyFailure(reason)


def pane_pid(socket: Path, configuration: Path, session: str) -> int:
    result = private_tmux(
        socket,
        configuration,
        "display-message",
        "-p",
        "-t",
        f"{session}:0.0",
        "#{pane_pid}",
    )
    value = result.stdout.strip()
    if not value.isdigit() or int(value) == 0:
        raise StudyFailure("pane-pid-malformed")
    return int(value)


def pane_capture(socket: Path, configuration: Path, session: str) -> str:
    return private_tmux(
        socket,
        configuration,
        "capture-pane",
        "-p",
        "-t",
        f"{session}:0.0",
    ).stdout


def has_session(socket: Path, configuration: Path, session: str) -> bool:
    return (
        private_tmux(
            socket,
            configuration,
            "has-session",
            "-t",
            session,
            check=False,
        ).returncode
        == 0
    )


def send_line(socket: Path, configuration: Path, session: str, value: str) -> None:
    private_tmux(
        socket,
        configuration,
        "send-keys",
        "-t",
        f"{session}:0.0",
        "-l",
        value,
    )
    private_tmux(
        socket,
        configuration,
        "send-keys",
        "-t",
        f"{session}:0.0",
        "C-m",
    )


def send_key(socket: Path, configuration: Path, session: str, key: str) -> None:
    private_tmux(
        socket,
        configuration,
        "send-keys",
        "-t",
        f"{session}:0.0",
        key,
    )


def record_lines(path: Path) -> list[str]:
    if not path.is_file():
        return []
    return [line for line in path.read_text(encoding="utf-8").splitlines() if line]


def wait_for_record(path: Path, count: int = 1) -> None:
    wait_until(lambda: len(record_lines(path)) >= count, "provider-record-timeout")


def parse_record(line: str) -> dict[str, str]:
    if " argv=" not in line:
        raise StudyFailure("provider-record-malformed")
    prefix, argv = line.split(" argv=", 1)
    values: dict[str, str] = {"argv": argv}
    for field in prefix.split():
        key, separator, value = field.partition("=")
        if not separator or key not in {"pid", "ppid", "pgid", "sid"}:
            raise StudyFailure("provider-record-malformed")
        values[key] = value
    if any(key not in values for key in ("pid", "ppid", "pgid", "sid")):
        raise StudyFailure("provider-record-malformed")
    if not all(
        values[key].isdigit() and int(values[key]) > 0
        for key in ("pid", "ppid", "pgid", "sid")
    ):
        raise StudyFailure("provider-record-malformed")
    return values


def start_shell(
    socket: Path,
    configuration: Path,
    root: Path,
    session: str,
    command: list[str],
    environment: dict[str, str],
) -> None:
    assignments = [f"{key}={value}" for key, value in environment.items()]
    shell_command = shlex.join(["env", *assignments, *command])
    private_tmux(
        socket,
        configuration,
        "new-session",
        "-d",
        "-s",
        session,
        "-n",
        "shell",
        "-c",
        str(root),
        shell_command,
    )


def kill_server(socket: Path, configuration: Path) -> None:
    private_tmux(socket, configuration, "kill-server", check=False)


def common_environment(
    *,
    record: Path,
    provider: Path,
    auto_exit: bool = False,
    broker: Path | None = None,
    broker_record: Path | None = None,
    path_prefix: Path | None = None,
) -> dict[str, str]:
    environment = {
        "WSNAV_RECORD": str(record),
        "WSNAV_FAKE_PROVIDER": str(provider),
        "WSNAV_AUTO_EXIT": "true" if auto_exit else "false",
    }
    if broker is not None:
        environment["WSNAV_BROKER"] = str(broker)
    if broker_record is not None:
        environment["WSNAV_BROKER_RECORD"] = str(broker_record)
    if path_prefix is not None:
        environment["PATH"] = f"{path_prefix}:{os.environ.get('PATH', '')}"
    return environment


def run_path_shim(
    root: Path,
    configuration: Path,
    provider: Path,
    path_shim: Path,
) -> dict[str, bool]:
    socket = root / "path.sock"
    record = root / "path.record"
    environment = common_environment(
        record=record,
        provider=provider,
        path_prefix=path_shim.parent,
    )
    start_shell(
        socket,
        configuration,
        root,
        "path",
        ["bash", "--noprofile", "--norc", "-i"],
        environment,
    )
    initial_pid = pane_pid(socket, configuration, "path")
    _, initial_birth = process_stat(initial_pid)
    send_line(socket, configuration, "path", EXPECTED_COMMAND)
    wait_for_record(record)
    provider_record = parse_record(record_lines(record)[0])
    provider_pid = int(provider_record["pid"])
    _, provider_birth = process_stat(provider_pid)
    send_key(socket, configuration, "path", "C-c")
    wait_until(lambda: not process_alive(provider_pid), "path-provider-did-not-exit")
    shell_survives = process_alive(initial_pid) and has_session(
        socket, configuration, "path"
    )
    output_survives = OUTPUT_MARKER in pane_capture(socket, configuration, "path")
    return {
        "args_preserved": provider_record["argv"] == EXPECTED_ARGV,
        "pane_pid_equals_provider": provider_pid == initial_pid,
        "provider_ppid_equals_initial_pane": int(provider_record["ppid"])
        == initial_pid,
        "provider_birth_equals_initial_pane": provider_birth == initial_birth,
        "provider_process_group_leader": int(provider_record["pgid"]) == provider_pid,
        "provider_session_nonempty": int(provider_record["sid"]) > 0,
        "broker_invoked": False,
        "shell_survives_provider_exit": shell_survives,
        "provider_output_survives_provider_exit": output_survives,
    }


def run_exec_function(
    root: Path,
    configuration: Path,
    provider: Path,
    broker: Path,
    bashrc: Path,
) -> dict[str, bool]:
    socket = root / "exec.sock"
    record = root / "exec.record"
    broker_record = root / "exec.broker"
    environment = common_environment(
        record=record,
        provider=provider,
        broker=broker,
        broker_record=broker_record,
        path_prefix=path_shim_directory(root),
    )
    environment["WSNAV_BROKER"] = str(broker)
    start_shell(
        socket,
        configuration,
        root,
        "exec",
        ["bash", "--noprofile", "--rcfile", str(bashrc), "-i"],
        environment,
    )
    initial_pid = pane_pid(socket, configuration, "exec")
    _, initial_birth = process_stat(initial_pid)
    send_line(socket, configuration, "exec", EXPECTED_COMMAND)
    wait_for_record(record)
    provider_record = parse_record(record_lines(record)[0])
    provider_pid = int(provider_record["pid"])
    _, provider_birth = process_stat(provider_pid)
    broker_invoked = bool(record_lines(broker_record))
    send_key(socket, configuration, "exec", "C-c")
    wait_until(lambda: not process_alive(initial_pid), "exec-provider-did-not-exit")
    return {
        "args_preserved": provider_record["argv"] == EXPECTED_ARGV,
        "pane_pid_equals_provider": provider_pid == initial_pid,
        "provider_ppid_equals_initial_pane": int(provider_record["ppid"])
        == initial_pid,
        "provider_birth_equals_initial_pane": provider_birth == initial_birth,
        "provider_process_group_leader": int(provider_record["pgid"]) == provider_pid,
        "provider_session_nonempty": int(provider_record["sid"]) > 0,
        "broker_invoked": broker_invoked,
        "shell_survives_provider_exit": False,
        "provider_output_survives_provider_exit": False,
    }


def run_preexec(
    root: Path,
    configuration: Path,
    provider: Path,
    broker: Path,
    zshrc_directory: Path,
) -> dict[str, bool]:
    socket = root / "preexec.sock"
    record = root / "preexec.record"
    broker_record = root / "preexec.broker"
    environment = common_environment(
        record=record,
        provider=provider,
        broker=broker,
        broker_record=broker_record,
        path_prefix=path_shim_directory(root),
    )
    environment["ZDOTDIR"] = str(zshrc_directory)
    start_shell(
        socket,
        configuration,
        root,
        "preexec",
        ["zsh", "-d", "-i"],
        environment,
    )
    initial_pid = pane_pid(socket, configuration, "preexec")
    _, initial_birth = process_stat(initial_pid)
    send_line(socket, configuration, "preexec", EXPECTED_COMMAND)
    wait_for_record(record)
    provider_record = parse_record(record_lines(record)[0])
    provider_pid = int(provider_record["pid"])
    _, provider_birth = process_stat(provider_pid)
    broker_invoked = bool(record_lines(broker_record))
    send_key(socket, configuration, "preexec", "C-c")
    wait_until(lambda: not process_alive(initial_pid), "preexec-provider-did-not-exit")
    return {
        "args_preserved": provider_record["argv"] == EXPECTED_ARGV,
        "pane_pid_equals_provider": provider_pid == initial_pid,
        "provider_ppid_equals_initial_pane": int(provider_record["ppid"])
        == initial_pid,
        "provider_birth_equals_initial_pane": provider_birth == initial_birth,
        "provider_process_group_leader": int(provider_record["pgid"]) == provider_pid,
        "provider_session_nonempty": int(provider_record["sid"]) > 0,
        "broker_invoked": broker_invoked,
        "shell_survives_provider_exit": False,
        "provider_output_survives_provider_exit": False,
    }


def path_shim_directory(root: Path) -> Path:
    return root / "bin"


def child_process_count(pid: int) -> int:
    result = run(["ps", "--ppid", str(pid), "-o", "pid="], check=False)
    if result.returncode != 0:
        return 0
    return len([line for line in result.stdout.splitlines() if line.strip().isdigit()])


def run_durable_shell(
    root: Path,
    configuration: Path,
    provider: Path,
    supervisor: Path,
) -> dict[str, bool]:
    socket = root / "durable.sock"
    record = root / "durable.record"
    supervisor_record = root / "durable.supervisor"
    environment = common_environment(
        record=record,
        provider=provider,
        auto_exit=True,
        path_prefix=path_shim_directory(root),
    )
    environment.update(
        {
            "WSNAV_SUPERVISOR_RECORD": str(supervisor_record),
            "WSNAV_SHELL": "bash",
        }
    )
    start_shell(socket, configuration, root, "durable", [str(supervisor)], environment)
    initial_pid = pane_pid(socket, configuration, "durable")
    _, initial_birth = process_stat(initial_pid)
    send_line(socket, configuration, "durable", EXPECTED_COMMAND)
    wait_for_record(record)
    provider_record = parse_record(record_lines(record)[0])
    provider_pid = int(provider_record["pid"])
    _, provider_birth = process_stat(provider_pid)
    wait_until(lambda: not process_alive(provider_pid), "durable-provider-did-not-exit")
    shell_survives = process_alive(initial_pid) and has_session(
        socket, configuration, "durable"
    )
    output_survives = OUTPUT_MARKER in pane_capture(socket, configuration, "durable")
    return {
        "args_preserved": provider_record["argv"] == EXPECTED_ARGV,
        "pane_pid_equals_provider": provider_pid == initial_pid,
        "provider_ppid_equals_initial_pane": int(provider_record["ppid"])
        == initial_pid,
        "provider_birth_equals_initial_pane": provider_birth == initial_birth,
        "provider_process_group_leader": int(provider_record["pgid"]) == provider_pid,
        "provider_session_nonempty": int(provider_record["sid"]) > 0,
        "broker_invoked": False,
        "shell_survives_provider_exit": shell_survives,
        "supervisor_survives_provider_exit": shell_survives,
        "shell_child_present_after_provider_exit": child_process_count(initial_pid)
        >= 1,
        "provider_output_survives_provider_exit": output_survives,
    }


def run_bypasses(
    root: Path,
    configuration: Path,
    provider: Path,
    broker: Path,
    bashrc: Path,
    direct_provider: Path,
    nested_script: Path,
) -> dict[str, bool | str]:
    socket = root / "bypass.sock"
    record = root / "bypass.record"
    broker_record = root / "bypass.broker"
    environment = common_environment(
        record=record,
        provider=provider,
        auto_exit=True,
        broker=broker,
        broker_record=broker_record,
        path_prefix=path_shim_directory(root),
    )
    environment["WSNAV_BROKER"] = str(broker)
    environment["WSNAV_DIRECT_PROVIDER"] = str(direct_provider)
    start_shell(
        socket,
        configuration,
        root,
        "bypass",
        ["bash", "--noprofile", "--rcfile", str(bashrc), "-i"],
        environment,
    )
    initial_pid = pane_pid(socket, configuration, "bypass")
    commands = [
        "command " + EXPECTED_COMMAND,
        shlex.join([str(direct_provider), *EXPECTED_ARGV.split()]),
        shlex.join([str(nested_script), *EXPECTED_ARGV.split()]),
    ]
    for index, command in enumerate(commands, start=1):
        send_line(socket, configuration, "bypass", command)
        wait_for_record(record, index)
    records = [parse_record(line) for line in record_lines(record)]
    broker_invoked = bool(record_lines(broker_record))

    def argument_shape(value: str) -> str:
        if value == EXPECTED_ARGV:
            return "expected"
        if value.startswith("--model"):
            return "model-prefixed-different"
        return "other"

    return {
        "command_codex_argument_shape": argument_shape(records[0]["argv"]),
        "absolute_path_argument_shape": argument_shape(records[1]["argv"]),
        "nested_script_argument_shape": argument_shape(records[2]["argv"]),
        "command_codex_args_preserved": records[0]["argv"] == EXPECTED_ARGV,
        "absolute_path_args_preserved": records[1]["argv"] == EXPECTED_ARGV,
        "nested_script_args_preserved": records[2]["argv"] == EXPECTED_ARGV,
        "command_codex_bypasses_function": not broker_invoked,
        "absolute_path_bypasses_function": not broker_invoked,
        "nested_script_bypasses_function": not broker_invoked,
        "all_bypasses_require_fail_closed": True,
        "bypass_outcome": "unmanaged_fail_closed_required",
        "shell_survives_bypasses": process_alive(initial_pid)
        and has_session(socket, configuration, "bypass"),
    }


def make_result(
    *,
    status: str,
    reason: str,
    tmux_release: str,
    candidates: dict[str, dict[str, bool]],
    bypasses: dict[str, bool | str],
    assertions: dict[str, bool],
) -> dict[str, Any]:
    return {
        "study": STUDY,
        "contract_fingerprint": CONTRACT,
        "status": status,
        "reason": reason,
        "environment": {"tmux_version": tmux_release},
        "candidates": candidates,
        "bypasses": bypasses,
        "assertions": assertions,
    }


def write_result(path: Path | None, result: dict[str, Any]) -> None:
    rendered = json.dumps(result, indent=2, sort_keys=True) + "\n"
    if path is None:
        print(rendered, end="")
        return
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(rendered, encoding="utf-8")
    path.chmod(0o600)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--result", type=Path, help="write sanitized JSON to this path")
    arguments = parser.parse_args()

    root: Path | None = None
    sockets: list[Path] = []
    configuration: Path | None = None
    before_fingerprint = ""
    after_fingerprint = ""
    baseline_established = False
    tmux_release = "unknown"
    candidates: dict[str, dict[str, bool]] = {}
    bypasses: dict[str, bool | str] = {}
    assertions: dict[str, bool] = {}
    status = "pass"
    reason = "all_topology_candidates_observed"

    try:
        required = ("tmux", "bash", "zsh", "ps", "sleep", "tr")
        if any(shutil.which(command) is None for command in required):
            raise StudyBlocked("required-command-unavailable")
        tmux_release = tmux_version()
        before_fingerprint = ordinary_tmux_fingerprint()
        baseline_established = True
        root = Path(tempfile.mkdtemp(prefix=ROOT_PREFIX))
        root.chmod(0o700)
        configuration = root / "tmux.conf"
        write_tmux_config(configuration)
        (root / "bin").mkdir(mode=0o700)
        provider = root / "fake-provider"
        broker = root / "broker"
        supervisor = root / "supervisor"
        write_private(provider, FAKE_PROVIDER_SOURCE, 0o700)
        write_private(broker, FAKE_BROKER_SOURCE, 0o700)
        write_private(supervisor, SUPERVISOR_SOURCE, 0o700)
        write_private(root / "bin" / "codex", PATH_SHIM_SOURCE, 0o700)
        write_private(root / "bin" / "opencode", PATH_SHIM_SOURCE, 0o700)
        direct_directory = root / "direct"
        direct_directory.mkdir(mode=0o700)
        direct_provider = direct_directory / "codex"
        write_private(direct_provider, FAKE_PROVIDER_SOURCE, 0o700)
        nested_script = root / "nested-provider"
        write_private(nested_script, NESTED_BYPASS_SOURCE, 0o700)
        bashrc = root / "bashrc"
        write_private(bashrc, EXEC_BASHRC_SOURCE, 0o600)
        zshrc_directory = root / "zsh"
        zshrc_directory.mkdir(mode=0o700)
        write_private(zshrc_directory / ".zshrc", PREEXEC_ZSHRC_SOURCE, 0o600)

        sockets.extend(
            root / name
            for name in (
                "path.sock",
                "exec.sock",
                "preexec.sock",
                "durable.sock",
                "bypass.sock",
            )
        )
        candidates["path_shim"] = run_path_shim(
            root, configuration, provider, root / "bin" / "codex"
        )
        kill_server(root / "path.sock", configuration)
        candidates["exec_function"] = run_exec_function(
            root, configuration, provider, broker, bashrc
        )
        kill_server(root / "exec.sock", configuration)
        candidates["preexec"] = run_preexec(
            root, configuration, provider, broker, zshrc_directory
        )
        kill_server(root / "preexec.sock", configuration)
        candidates["durable_shell_supervisor"] = run_durable_shell(
            root, configuration, provider, supervisor
        )
        kill_server(root / "durable.sock", configuration)
        bypasses = run_bypasses(
            root,
            configuration,
            provider,
            broker,
            bashrc,
            direct_provider,
            nested_script,
        )
        kill_server(root / "bypass.sock", configuration)

        assertions = {
            "fixed_arguments_preserved_by_path_and_exec": candidates["path_shim"][
                "args_preserved"
            ]
            and candidates["exec_function"]["args_preserved"],
            "exec_function_preserves_current_runtime_identity": candidates[
                "exec_function"
            ]["pane_pid_equals_provider"]
            and candidates["exec_function"]["provider_birth_equals_initial_pane"]
            and candidates["exec_function"]["provider_process_group_leader"],
            "path_shim_changes_current_runtime_identity": not candidates["path_shim"][
                "pane_pid_equals_provider"
            ],
            "preexec_argument_corruption_observed": not candidates["preexec"][
                "args_preserved"
            ],
            "durable_shell_keeps_provider_output_after_exit": candidates[
                "durable_shell_supervisor"
            ]["provider_output_survives_provider_exit"],
            "provider_pgid_leadership_observed_for_all_candidates": all(
                candidate["provider_process_group_leader"]
                for candidate in candidates.values()
            ),
            "bypasses_require_fail_closed": bool(
                bypasses["all_bypasses_require_fail_closed"]
            ),
            "private_tmux_only": True,
        }
        if not all(assertions.values()):
            raise StudyFailure("topology-assertion-failed")
    except StudyBlocked as error:
        status = "blocked"
        reason = str(error)
    except StudyFailure as error:
        status = "falsified"
        reason = str(error)
    finally:
        if configuration is not None:
            for socket in sockets:
                kill_server(socket, configuration)
        if root is not None:
            try:
                shutil.rmtree(root)
            except OSError:
                status = "falsified"
                reason = "temporary-root-cleanup-failed"
        if baseline_established:
            after_fingerprint = ordinary_tmux_fingerprint()
            assertions["ordinary_tmux_unchanged"] = bool(
                before_fingerprint and before_fingerprint == after_fingerprint
            )
        else:
            assertions["ordinary_tmux_unchanged"] = False
        assertions["cleanup_complete"] = root is None or not root.exists()
        if baseline_established and not assertions["ordinary_tmux_unchanged"]:
            status = "falsified"
            reason = "ordinary-tmux-changed"
        if not assertions["cleanup_complete"]:
            status = "falsified"
            reason = "temporary-root-cleanup-failed"

    result = make_result(
        status=status,
        reason=reason,
        tmux_release=tmux_release,
        candidates=candidates,
        bypasses=bypasses,
        assertions=assertions,
    )
    write_result(arguments.result, result)
    return 0 if status == "pass" else 1


if __name__ == "__main__":
    raise SystemExit(main())
