#!/usr/bin/env python3
"""Accept D8.2 against real OpenCode through local and loopback-SSH WSNav paths.

This is operator-gated because it submits one harmless turn per path. All
provider homes, repositories, WSNav roots, SSH material, processes, ports, and
private tmux servers are disposable. The result contains bounded assertions
only; provider output and identifiers are discarded with the temporary root.
"""

from __future__ import annotations

import argparse
import getpass
import json
import os
import shlex
import shutil
import socket
import sqlite3
import subprocess
import sys
import tempfile
import time
from collections.abc import Callable
from pathlib import Path
from typing import Any

from opencode_support import environment_for_directory, isolated_environment

MODEL = "opencode-go/deepseek-v4-flash"
MARKER = "WSNAV_D82_ACCEPTANCE_RESULT"
PROMPT = (
    f"Reply with the exact token {MARKER} and nothing else. "
    "Do not use tools, inspect files, or make changes."
)


class AcceptanceFailure(RuntimeError):
    """A bounded product assertion failed."""


class AcceptanceBlocked(RuntimeError):
    """A required local acceptance prerequisite was unavailable."""


def run(
    arguments: list[str],
    *,
    cwd: Path | None = None,
    env: dict[str, str] | None = None,
    timeout: int = 180,
    check: bool = True,
) -> subprocess.CompletedProcess[str]:
    result = subprocess.run(
        arguments,
        cwd=cwd,
        env=env,
        timeout=timeout,
        capture_output=True,
        text=True,
        check=False,
    )
    if check and result.returncode != 0:
        raise AcceptanceFailure(f"command-failed:{Path(arguments[0]).name}")
    return result


def free_port() -> int:
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        return int(listener.getsockname()[1])


def wait_for(predicate: Callable[[], Any], label: str, timeout: int = 45) -> Any:
    deadline = time.time() + timeout
    while time.time() < deadline:
        try:
            value = predicate()
            if value:
                return value
        except (OSError, sqlite3.Error):
            pass
        time.sleep(0.25)
    raise AcceptanceFailure(f"timeout:{label}")


def create_repository(path: Path) -> None:
    path.mkdir(parents=True)
    run(["git", "init", "-q", "-b", "main"], cwd=path)
    run(["git", "config", "user.name", "wsnav-acceptance"], cwd=path)
    run(["git", "config", "user.email", "wsnav@example.test"], cwd=path)
    (path / "README").write_text("disposable\n", encoding="utf-8")
    run(["git", "add", "README"], cwd=path)
    run(["git", "commit", "-qm", "initial"], cwd=path)


def wsnav(
    binary: Path,
    state_root: Path,
    *arguments: str,
    env: dict[str, str],
    check: bool = True,
) -> subprocess.CompletedProcess[str]:
    result = run(
        [str(binary), "--state-root", str(state_root), *arguments],
        env=env,
        check=False,
    )
    if check and result.returncode != 0:
        label = arguments[0]
        if label == "host" and len(arguments) > 1:
            label = f"host-{arguments[1]}"
        raise AcceptanceFailure(f"wsnav-command-failed:{label}")
    return result


def output_id(output: str) -> str:
    value = output.strip().rsplit(" ", 1)[-1]
    if not value or any(character.isspace() for character in value):
        raise AcceptanceFailure("invalid-workstream-output")
    return value


def runtime_info(state_root: Path, workstream_id: str) -> dict[str, Any] | None:
    database = state_root / "host.sqlite"
    if not database.exists():
        return None
    with sqlite3.connect(database) as connection:
        row = connection.execute(
            """SELECT w.revision, w.lifecycle, w.provider, w.source_workstream_id,
                      r.runtime_id, r.lifecycle, r.tmux_generation,
                      b.native_session_id, b.last_settled_turn_id,
                      h.runtime_generation, h.endpoint_port, h.observer_pid,
                      h.observer_birth, h.observer_status
                 FROM workstreams w
                 LEFT JOIN runtimes r ON r.workstream_id = w.workstream_id
                 LEFT JOIN provider_bindings b ON b.runtime_id = r.runtime_id
                 LEFT JOIN opencode_runtime_handles h ON h.runtime_id = r.runtime_id
                WHERE w.workstream_id = ?""",
            (workstream_id,),
        ).fetchone()
    if row is None:
        return None
    keys = (
        "revision",
        "workstream_lifecycle",
        "provider",
        "source_workstream_id",
        "runtime_id",
        "runtime_lifecycle",
        "tmux_generation",
        "session_id",
        "settled_id",
        "handle_generation",
        "port",
        "observer_pid",
        "observer_birth",
        "observer_status",
    )
    return dict(zip(keys, row, strict=True))


def ready_runtime(state_root: Path, workstream_id: str) -> dict[str, Any] | None:
    info = runtime_info(state_root, workstream_id)
    if (
        info is not None
        and info["provider"] == "opencode"
        and info["session_id"]
        and info["observer_status"] == "ready"
        and info["port"]
    ):
        return info
    return None


def submit_turn(env: dict[str, str], project: Path, info: dict[str, Any]) -> None:
    endpoint = f"http://127.0.0.1:{info['port']}"
    result = run(
        [
            "opencode",
            "--pure",
            "run",
            "--attach",
            endpoint,
            "--session",
            str(info["session_id"]),
            "--model",
            MODEL,
            "--format",
            "json",
            PROMPT,
        ],
        cwd=project,
        env=environment_for_directory(env, project),
        timeout=180,
        check=False,
    )
    if result.returncode != 0 or MARKER not in result.stdout:
        raise AcceptanceBlocked("real-opencode-turn-unavailable")


def private_socket(state_root: Path, runtime_id: str) -> Path:
    return state_root / "run" / f"runtime-{runtime_id}" / "tmux.sock"


def kill_private_runtime(state_root: Path, runtime_id: str) -> None:
    environment = os.environ.copy()
    environment.pop("TMUX", None)
    run(
        ["tmux", "-S", str(private_socket(state_root, runtime_id)), "kill-server"],
        env=environment,
    )


def pid_is_gone(pid: int) -> bool:
    stat = Path(f"/proc/{pid}/stat")
    if not stat.exists():
        return True
    try:
        state = stat.read_text(encoding="utf-8").rsplit(")", 1)[-1].split()[0]
    except (OSError, IndexError):
        return True
    return state == "Z"


def port_is_closed(port: int) -> bool:
    try:
        with socket.create_connection(("127.0.0.1", port), timeout=0.25):
            return False
    except OSError:
        return True


def assert_state_has_no_marker(*roots: Path) -> None:
    marker = MARKER.encode()
    for root in roots:
        for path in root.rglob("*") if root.exists() else ():
            if not path.is_file() or path.stat().st_size > 16 * 1024 * 1024:
                continue
            if marker in path.read_bytes():
                raise AcceptanceFailure("provider-content-entered-wsnav-state")


def park_direct(
    binary: Path,
    state_root: Path,
    workstream_id: str,
    env: dict[str, str],
) -> None:
    wsnav(binary, state_root, "park", workstream_id, env=env, check=False)


def accept_host_path(
    *,
    binary: Path,
    provider_env: dict[str, str],
    project: Path,
    host_state: Path,
    invoke: Callable[[list[str]], subprocess.CompletedProcess[str]],
    register: Callable[[], subprocess.CompletedProcess[str]],
    reconcile: Callable[[str], None],
    assertions: dict[str, bool],
    prefix: str,
) -> tuple[str, str]:
    source_id = output_id(register().stdout)
    info = runtime_info(host_state, source_id)
    if info is None:
        raise AcceptanceFailure(f"{prefix}-registration-missing")
    invoke(["start", source_id, str(info["revision"])])
    source = wait_for(
        lambda: ready_runtime(host_state, source_id), f"{prefix}-source-ready"
    )
    submit_turn(provider_env, project, source)
    source = wait_for(
        lambda: (
            current
            if (current := ready_runtime(host_state, source_id))
            and current["settled_id"]
            else None
        ),
        f"{prefix}-settled-boundary",
    )

    forked = invoke(["fork", source_id, str(source["revision"])])
    destination_id = output_id(forked.stdout)
    destination = wait_for(
        lambda: ready_runtime(host_state, destination_id),
        f"{prefix}-destination-ready",
    )
    if (
        destination["provider"] != "opencode"
        or destination["source_workstream_id"] != source_id
        or destination["session_id"] == source["session_id"]
    ):
        raise AcceptanceFailure(f"{prefix}-fork-identity")
    with sqlite3.connect(host_state / "host.sqlite") as connection:
        operation = connection.execute(
            "SELECT phase FROM compound_operations ORDER BY rowid DESC LIMIT 1"
        ).fetchone()
    if operation != ("committed",):
        raise AcceptanceFailure(f"{prefix}-fork-not-committed")
    assertions[f"{prefix}_fork_exact_session"] = True

    invoke(["park", destination_id, str(destination["revision"])])
    wait_for(
        lambda: (
            runtime_info(host_state, destination_id)["runtime_lifecycle"] == "stopped"
        ),
        f"{prefix}-destination-parked",
    )

    before = ready_runtime(host_state, source_id)
    if before is None:
        raise AcceptanceFailure(f"{prefix}-source-not-ready-before-loss")
    kill_private_runtime(host_state, str(before["runtime_id"]))
    reconcile(source_id)
    lost = wait_for(
        lambda: (
            current
            if (current := runtime_info(host_state, source_id))
            and current["workstream_lifecycle"] == "recovery_required"
            and current["runtime_lifecycle"] == "unknown"
            else None
        ),
        f"{prefix}-loss-reconciled",
    )
    invoke(["recover", source_id, str(lost["revision"])])
    recovered = wait_for(
        lambda: (
            current
            if (current := ready_runtime(host_state, source_id))
            and current["workstream_lifecycle"] == "open"
            else None
        ),
        f"{prefix}-recovered",
    )
    if recovered["session_id"] != before["session_id"]:
        raise AcceptanceFailure(f"{prefix}-recovery-session-changed")
    if (
        recovered["handle_generation"] == before["handle_generation"]
        or recovered["port"] == before["port"]
        or recovered["observer_pid"] == before["observer_pid"]
    ):
        raise AcceptanceFailure(f"{prefix}-recovery-generation-not-replaced")
    if not wait_for(
        lambda: pid_is_gone(int(before["observer_pid"])),
        f"{prefix}-old-observer-stopped",
    ) or not wait_for(
        lambda: port_is_closed(int(before["port"])), f"{prefix}-old-port-closed"
    ):
        raise AcceptanceFailure(f"{prefix}-prior-runtime-not-clean")
    assertions[f"{prefix}_recovery_exact_session"] = True
    assertions[f"{prefix}_generation_replaced"] = True
    return source_id, destination_id


def tmux_snapshot() -> str:
    environment = os.environ.copy()
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
        env=environment,
        check=False,
    )
    return result.stdout if result.returncode == 0 else ""


def write_executable(path: Path, content: str) -> None:
    path.write_text(content, encoding="utf-8")
    path.chmod(0o700)


def start_sshd(root: Path, client_env: dict[str, str]) -> subprocess.Popen[str]:
    port = free_port()
    host_key = root / "ssh-host-key"
    client_key = root / "ssh-client-key"
    authorized = root / "authorized_keys"
    run(["ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-f", str(host_key)])
    run(["ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-f", str(client_key)])
    authorized.write_text(
        client_key.with_suffix(".pub").read_text(encoding="utf-8"), encoding="utf-8"
    )
    authorized.chmod(0o600)
    sshd_config = root / "sshd_config"
    sshd_config.write_text(
        "\n".join(
            (
                f"Port {port}",
                "ListenAddress 127.0.0.1",
                f"HostKey {host_key}",
                f"PidFile {root / 'sshd.pid'}",
                f"AuthorizedKeysFile {authorized}",
                "PubkeyAuthentication yes",
                "PasswordAuthentication no",
                "KbdInteractiveAuthentication no",
                "UsePAM no",
                "PermitRootLogin no",
                "StrictModes no",
                "LogLevel ERROR",
            )
        )
        + "\n",
        encoding="utf-8",
    )
    client_config = root / "ssh_config"
    client_config.write_text(
        "\n".join(
            (
                "Host d82-loopback",
                "HostName 127.0.0.1",
                f"Port {port}",
                f"User {getpass.getuser()}",
                f"IdentityFile {client_key}",
                "IdentitiesOnly yes",
                "BatchMode yes",
                "StrictHostKeyChecking no",
                "UserKnownHostsFile /dev/null",
                "LogLevel ERROR",
            )
        )
        + "\n",
        encoding="utf-8",
    )
    ssh_bin = root / "client-bin"
    ssh_bin.mkdir()
    write_executable(
        ssh_bin / "ssh",
        "#!/usr/bin/env bash\n"
        f'exec /usr/bin/ssh -F {shlex.quote(str(client_config))} "$@"\n',
    )
    client_env["PATH"] = f"{ssh_bin}:{client_env['PATH']}"
    process = subprocess.Popen(
        ["/usr/bin/sshd", "-D", "-e", "-f", str(sshd_config)],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.PIPE,
        text=True,
    )
    try:
        wait_for(
            lambda: (
                run(
                    ["ssh", "d82-loopback", "true"],
                    env=client_env,
                    timeout=5,
                    check=False,
                ).returncode
                == 0
            ),
            "loopback-sshd",
        )
    except Exception:
        process.terminate()
        process.wait(timeout=10)
        raise
    return process


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--confirm-live-opencode", action="store_true")
    parser.add_argument("--result", type=Path)
    args = parser.parse_args()

    assertions = {
        "operator_confirmed": args.confirm_live_opencode,
        "local_fork_exact_session": False,
        "local_recovery_exact_session": False,
        "local_generation_replaced": False,
        "local_cleanup": False,
        "ssh_fork_exact_session": False,
        "ssh_recovery_exact_session": False,
        "ssh_generation_replaced": False,
        "ssh_cleanup": False,
        "wsnav_state_excludes_provider_content": False,
        "ordinary_tmux_unchanged": False,
        "cleanup_complete": False,
    }
    status = "blocked"
    reason = "operator-confirmation-required"
    root: Path | None = None
    sshd: subprocess.Popen[str] | None = None
    cleanup_targets: list[tuple[Path, str, dict[str, str]]] = []
    ordinary_before = tmux_snapshot()
    try:
        if not args.confirm_live_opencode:
            raise AcceptanceBlocked(reason)
        repository_root = Path(__file__).resolve().parents[1]
        binary = repository_root / "target" / "debug" / "wsnav"
        if not binary.is_file():
            raise AcceptanceBlocked("candidate-binary-missing")
        version = run(["opencode", "--version"]).stdout.strip()
        if version != "1.18.11":
            raise AcceptanceBlocked("opencode-version-not-allowlisted")
        root = Path(tempfile.mkdtemp(prefix="wd82."))

        local_root = root / "local"
        local_env = isolated_environment(local_root / "provider")
        local_state = local_root / "state"
        local_project = local_root / "project"
        create_repository(local_project)

        def local_register() -> subprocess.CompletedProcess[str]:
            registered = wsnav(
                binary,
                local_state,
                "register",
                "--provider",
                "opencode",
                str(local_project),
                env=local_env,
            )
            source_id = output_id(registered.stdout)
            cleanup_targets.append((local_state, source_id, local_env))
            return registered

        def local_invoke(arguments: list[str]) -> subprocess.CompletedProcess[str]:
            action, workstream_id, *_ = arguments
            command = {
                "start": ["start", workstream_id],
                "fork": ["fork-workstream", workstream_id],
                "park": ["park", workstream_id],
                "recover": ["recover", workstream_id],
            }[action]
            result = wsnav(binary, local_state, *command, env=local_env)
            if action == "fork":
                cleanup_targets.append(
                    (local_state, output_id(result.stdout), local_env)
                )
            return result

        def local_reconcile(workstream_id: str) -> None:
            wsnav(binary, local_state, "status", workstream_id, env=local_env)

        local_source, local_destination = accept_host_path(
            binary=binary,
            provider_env=local_env,
            project=local_project,
            host_state=local_state,
            invoke=local_invoke,
            register=local_register,
            reconcile=local_reconcile,
            assertions=assertions,
            prefix="local",
        )
        park_direct(binary, local_state, local_source, local_env)
        park_direct(binary, local_state, local_destination, local_env)
        wait_for(
            lambda: not list((local_state / "run").rglob("tmux.sock")),
            "local-private-sockets-clean",
        )
        assertions["local_cleanup"] = True

        remote_root = root / "remote"
        remote_env = isolated_environment(remote_root / "provider")
        remote_state = remote_root / "state"
        remote_project = remote_root / "project"
        client_state = root / "client-state"
        client_env = os.environ.copy()
        create_repository(remote_project)
        remote_wrapper = root / "remote-wsnav"
        exports = "\n".join(
            f"export {name}={shlex.quote(remote_env[name])}"
            for name in (
                "XDG_CONFIG_HOME",
                "XDG_DATA_HOME",
                "XDG_CACHE_HOME",
                "XDG_STATE_HOME",
            )
        )
        write_executable(
            remote_wrapper,
            "#!/usr/bin/env bash\n"
            "set -euo pipefail\n"
            f"{exports}\n"
            f"export PATH={shlex.quote(remote_env['PATH'])}\n"
            f"exec {shlex.quote(str(binary))} --state-root "
            f'{shlex.quote(str(remote_state))} "$@"\n',
        )
        sshd = start_sshd(root, client_env)
        wsnav(
            binary,
            client_state,
            "register-remote",
            "remote",
            "--destination",
            "d82-loopback",
            "--executable",
            str(remote_wrapper),
            env=client_env,
        )

        def remote_register() -> subprocess.CompletedProcess[str]:
            registered = wsnav(
                binary,
                client_state,
                "host",
                "register-checkout",
                "remote",
                str(remote_project),
                "--provider",
                "opencode",
                env=client_env,
            )
            source_id = output_id(registered.stdout)
            cleanup_targets.append((remote_state, source_id, remote_env))
            return registered

        def remote_invoke(arguments: list[str]) -> subprocess.CompletedProcess[str]:
            action, workstream_id, revision = arguments
            command = ["host", action, "remote", workstream_id, revision]
            result = wsnav(binary, client_state, *command, env=client_env, check=False)
            if (
                result.returncode != 0
                and action == "fork"
                and "revision conflict; refresh this host" in result.stderr
            ):
                current = runtime_info(remote_state, workstream_id)
                if current is None:
                    raise AcceptanceFailure("ssh-fork-refresh-state-missing")
                command[-1] = str(current["revision"])
                result = wsnav(binary, client_state, *command, env=client_env)
            elif result.returncode != 0:
                raise AcceptanceFailure(f"wsnav-command-failed:host-{action}")
            if action == "fork":
                cleanup_targets.append(
                    (remote_state, output_id(result.stdout), remote_env)
                )
            return result

        def remote_reconcile(workstream_id: str) -> None:
            current = runtime_info(remote_state, workstream_id)
            if current is None:
                raise AcceptanceFailure("ssh-loss-state-missing")
            result = wsnav(
                binary,
                client_state,
                "host",
                "start",
                "remote",
                workstream_id,
                str(current["revision"]),
                env=client_env,
                check=False,
            )
            if result.returncode == 0:
                raise AcceptanceFailure("ssh-lost-runtime-started-without-recovery")

        remote_source, remote_destination = accept_host_path(
            binary=binary,
            provider_env=remote_env,
            project=remote_project,
            host_state=remote_state,
            invoke=remote_invoke,
            register=remote_register,
            reconcile=remote_reconcile,
            assertions=assertions,
            prefix="ssh",
        )
        remote_source_info = runtime_info(remote_state, remote_source)
        remote_destination_info = runtime_info(remote_state, remote_destination)
        if remote_source_info is None or remote_destination_info is None:
            raise AcceptanceFailure("ssh-cleanup-state-missing")
        remote_invoke(["park", remote_source, str(remote_source_info["revision"])])
        wait_for(
            lambda: not list((remote_state / "run").rglob("tmux.sock")),
            "ssh-private-sockets-clean",
        )
        assertions["ssh_cleanup"] = True

        assert_state_has_no_marker(local_state, remote_state, client_state)
        assertions["wsnav_state_excludes_provider_content"] = True
        status = "pass"
        reason = "d8.2-real-local-and-ssh-acceptance-passed"
    except AcceptanceBlocked as error:
        status, reason = "blocked", str(error)
    except AcceptanceFailure as error:
        status, reason = "falsified", str(error)
    except (OSError, sqlite3.Error, subprocess.TimeoutExpired) as error:
        status, reason = "blocked", f"harness-error:{type(error).__name__}"
    finally:
        if root is not None:
            binary = Path(__file__).resolve().parents[1] / "target" / "debug" / "wsnav"
            for state_root, workstream_id, environment in reversed(cleanup_targets):
                park_direct(binary, state_root, workstream_id, environment)
            for socket_path in root.rglob("tmux.sock"):
                environment = os.environ.copy()
                environment.pop("TMUX", None)
                run(
                    ["tmux", "-S", str(socket_path), "kill-server"],
                    env=environment,
                    check=False,
                )
        if sshd is not None and sshd.poll() is None:
            sshd.terminate()
            try:
                sshd.wait(timeout=10)
            except subprocess.TimeoutExpired:
                sshd.kill()
                sshd.wait(timeout=10)
        if root is not None:
            shutil.rmtree(root, ignore_errors=True)
            assertions["cleanup_complete"] = not root.exists()
        assertions["ordinary_tmux_unchanged"] = tmux_snapshot() == ordinary_before

    result = {
        "study": "opencode-production-d8.2",
        "status": status,
        "reason": reason,
        "versions": {"opencode": "1.18.11"},
        "assertions": assertions,
    }
    rendered = json.dumps(result, indent=2) + "\n"
    if args.result:
        args.result.write_text(rendered, encoding="utf-8")
        args.result.chmod(0o600)
    else:
        sys.stdout.write(rendered)
    return 0 if status == "pass" else 1


if __name__ == "__main__":
    raise SystemExit(main())
