#!/usr/bin/env python3
"""Exercise the D17 prepare-capability-helper-exec boundary synthetically.

The harness starts no real provider and touches no ordinary WSNav state.  Four
controlled Bash/Zsh shells run in disposable private tmux servers.  Their
provider functions invoke this file as a bounded prepare child, capture only a
one-shot opaque capability, and exec this file as the hidden helper.  The
helper consumes the capability while holding a CLOEXEC lease FD and then execs
a fixed fake provider.

Only bounded booleans, enums, and tool versions leave the temporary root.
Process identities, paths, capabilities, arguments, and transient records are
deleted before the sanitized result is written.
"""

from __future__ import annotations

import argparse
import fcntl
import hashlib
import json
import os
import secrets
import shlex
import shutil
import signal
import stat
import subprocess
import sys
import tempfile
import time
import uuid
from pathlib import Path
from typing import Any, Final

STUDY: Final = "d17-two-phase-handshake"
CONTRACT: Final = "prepare-capability-helper-exec-v1"
ROOT_PREFIX: Final = "wsnav-d17-handshake."
COMMAND_TIMEOUT_SECONDS: Final = 4.0
WAIT_TIMEOUT_SECONDS: Final = 4.0
POLL_SECONDS: Final = 0.03
CAPABILITY_TTL_NS: Final = 5_000_000_000
MAX_RECORD_BYTES: Final = 32 * 1024
PROVIDERS: Final = ("codex", "opencode")
EXPECTED_PROVIDER_ARGS: Final = ("--model", "demo", "--flag", "value")
OUTPUT_MARKER: Final = "WSNAV_SYNTHETIC_PROVIDER_OUTPUT"
CONTEXT_KEYS: Final = (
    "candidate_runtime_id",
    "lease_generation",
    "presentation_id",
    "presentation_revision",
    "registry_revision",
    "request_id",
    "runtime_generation",
    "runtime_paths_digest",
    "slot_generation",
)
CLAIM_KEYS: Final = (
    *CONTEXT_KEYS,
    "argv_digest",
    "cwd_digest",
    "leader_birth",
    "leader_pgid",
    "leader_pid",
    "leader_sid",
    "provider",
    "root_digest",
)


class SpikeFailure(RuntimeError):
    """The synthetic result contradicted the expected contract."""


class SpikeBlocked(RuntimeError):
    """The disposable study could not start safely."""


class CapabilityRejected(RuntimeError):
    """A bounded capability check failed closed."""


WRAPPER_SOURCE: Final = r"""
wsnav_launch() {
    local wsnav_provider="$1"
    shift
    local wsnav_capability
    if ! wsnav_capability="$(
        "${WSNAV_PYTHON:?}" "${WSNAV_SPIKE:?}" --internal prepare \
            --provider "$wsnav_provider" --leader-pid "$$" -- "$@"
    )"; then
        return 64
    fi
    exec "${WSNAV_PYTHON:?}" "${WSNAV_SPIKE:?}" --internal helper \
        --provider "$wsnav_provider" \
        --capability "$wsnav_capability" -- "$@"
}
unalias codex opencode 2>/dev/null || true
unset -f codex opencode 2>/dev/null || true
codex() { wsnav_launch codex "$@"; }
opencode() { wsnav_launch opencode "$@"; }
"""


def run(
    arguments: list[str],
    *,
    environment: dict[str, str] | None = None,
    check: bool = False,
    timeout: float = COMMAND_TIMEOUT_SECONDS,
    pass_fds: tuple[int, ...] = (),
) -> subprocess.CompletedProcess[str]:
    try:
        result = subprocess.run(
            arguments,
            capture_output=True,
            check=False,
            env=environment,
            pass_fds=pass_fds,
            text=True,
            timeout=timeout,
        )
    except subprocess.TimeoutExpired as error:
        raise SpikeFailure("subprocess-timeout") from error
    except OSError as error:
        raise SpikeBlocked("required-subprocess-unavailable") from error
    if check and result.returncode != 0:
        raise SpikeFailure("subprocess-failed")
    return result


def private_tmux(
    socket: Path,
    configuration: Path,
    *arguments: str,
    environment: dict[str, str] | None = None,
    check: bool = True,
) -> subprocess.CompletedProcess[str]:
    tmux_environment = dict(os.environ if environment is None else environment)
    tmux_environment.pop("TMUX", None)
    return run(
        ["tmux", "-f", str(configuration), "-S", str(socket), *arguments],
        environment=tmux_environment,
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
    )
    if result.returncode != 0:
        return "absent"
    return hashlib.sha256(result.stdout.encode("utf-8")).hexdigest()


def tool_version(command: str, argument: str) -> str:
    result = run([command, argument])
    if result.returncode != 0 or not result.stdout.strip():
        raise SpikeBlocked(f"{command}-version-unavailable")
    first_line = result.stdout.splitlines()[0].strip()
    if len(first_line) > 160:
        raise SpikeFailure(f"{command}-version-malformed")
    return first_line


def write_private(path: Path, content: str, mode: int) -> None:
    path.write_text(content, encoding="utf-8")
    path.chmod(mode)


def fsync_directory(path: Path) -> None:
    flags = os.O_RDONLY | os.O_CLOEXEC
    if hasattr(os, "O_DIRECTORY"):
        flags |= os.O_DIRECTORY
    descriptor = os.open(path, flags)
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def write_json_atomic(path: Path, value: dict[str, Any]) -> None:
    encoded = (json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n").encode(
        "utf-8"
    )
    if len(encoded) > MAX_RECORD_BYTES:
        raise SpikeFailure("record-too-large")
    temporary = path.with_name(f".{path.name}.{os.getpid()}.{secrets.token_hex(4)}")
    descriptor = os.open(
        temporary,
        os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_CLOEXEC,
        0o600,
    )
    try:
        view = memoryview(encoded)
        while view:
            written = os.write(descriptor, view)
            if written <= 0:
                raise SpikeFailure("record-write-failed")
            view = view[written:]
        os.fsync(descriptor)
    finally:
        os.close(descriptor)
    os.replace(temporary, path)
    fsync_directory(path.parent)


def read_json_bounded(path: Path) -> dict[str, Any]:
    flags = os.O_RDONLY | os.O_CLOEXEC
    if hasattr(os, "O_NOFOLLOW"):
        flags |= os.O_NOFOLLOW
    descriptor = os.open(path, flags)
    try:
        metadata = os.fstat(descriptor)
        if not stat.S_ISREG(metadata.st_mode):
            raise CapabilityRejected("capability-record-unsafe")
        if metadata.st_uid != os.getuid() or stat.S_IMODE(metadata.st_mode) != 0o600:
            raise CapabilityRejected("capability-record-unsafe")
        encoded = os.read(descriptor, MAX_RECORD_BYTES + 1)
    finally:
        os.close(descriptor)
    if len(encoded) > MAX_RECORD_BYTES:
        raise CapabilityRejected("capability-record-oversized")
    try:
        value = json.loads(encoded)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise CapabilityRejected("capability-record-malformed") from error
    if not isinstance(value, dict):
        raise CapabilityRejected("capability-record-malformed")
    return value


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


def digest_text(value: str) -> str:
    return hashlib.sha256(value.encode("utf-8")).hexdigest()


def digest_argv(values: tuple[str, ...]) -> str:
    encoded = json.dumps(list(values), separators=(",", ":"), ensure_ascii=True)
    return digest_text(encoded)


def process_identity(pid: int) -> dict[str, int | str]:
    try:
        value = Path(f"/proc/{pid}/stat").read_text(encoding="utf-8")
        fields = value[value.rfind(")") + 2 :].split()
        state = fields[0]
        identity: dict[str, int | str] = {
            "state": state,
            "ppid": int(fields[1]),
            "pgid": int(fields[2]),
            "sid": int(fields[3]),
            "birth": fields[19],
        }
    except (FileNotFoundError, IndexError, OSError, ValueError) as error:
        raise CapabilityRejected("process-identity-unavailable") from error
    if state == "Z":
        raise CapabilityRejected("process-not-live")
    return identity


def process_alive(pid: int) -> bool:
    try:
        return process_identity(pid)["state"] != "Z"
    except CapabilityRejected:
        return False


def git_root(cwd: Path) -> Path:
    top = run(["git", "-C", str(cwd), "rev-parse", "--show-toplevel"])
    bare = run(["git", "-C", str(cwd), "rev-parse", "--is-bare-repository"])
    if top.returncode != 0 or bare.returncode != 0 or bare.stdout.strip() != "false":
        raise CapabilityRejected("non-bare-git-root-required")
    root = Path(top.stdout.strip()).resolve(strict=True)
    resolved_cwd = cwd.resolve(strict=True)
    try:
        resolved_cwd.relative_to(root)
    except ValueError as error:
        raise CapabilityRejected("git-root-mismatch") from error
    return root


def context_claims() -> dict[str, str]:
    raw = os.environ.get("WSNAV_CONTEXT", "")
    try:
        value = json.loads(raw)
    except json.JSONDecodeError as error:
        raise CapabilityRejected("context-malformed") from error
    if not isinstance(value, dict) or set(value) != set(CONTEXT_KEYS):
        raise CapabilityRejected("context-malformed")
    if any(
        not isinstance(value[key], str) or not value[key] or len(value[key]) > 128
        for key in CONTEXT_KEYS
    ):
        raise CapabilityRejected("context-malformed")
    return {key: value[key] for key in CONTEXT_KEYS}


def approve_provider_argv(provider: str, values: tuple[str, ...]) -> None:
    if provider not in PROVIDERS:
        raise CapabilityRejected("provider-unsupported")
    if values != EXPECTED_PROVIDER_ARGS:
        raise CapabilityRejected("grammar-rejected")


def build_claims(
    provider: str,
    provider_args: tuple[str, ...],
    leader_pid: int,
) -> dict[str, str | int]:
    approve_provider_argv(provider, provider_args)
    cwd = Path.cwd().resolve(strict=True)
    root = git_root(cwd)
    identity = process_identity(leader_pid)
    if int(identity["pgid"]) != leader_pid:
        raise CapabilityRejected("shell-not-process-group-leader")
    claims: dict[str, str | int] = context_claims()
    claims.update(
        {
            "argv_digest": digest_argv(provider_args),
            "cwd_digest": digest_text(str(cwd)),
            "leader_birth": str(identity["birth"]),
            "leader_pgid": int(identity["pgid"]),
            "leader_pid": leader_pid,
            "leader_sid": int(identity["sid"]),
            "provider": provider,
            "root_digest": digest_text(str(root)),
        }
    )
    if set(claims) != set(CLAIM_KEYS):
        raise SpikeFailure("claim-shape-invalid")
    return claims


def state_paths() -> tuple[Path, Path, Path]:
    raw_root = os.environ.get("WSNAV_STATE_ROOT", "")
    raw_lock = os.environ.get("WSNAV_LOCK_PATH", "")
    if not raw_root or not raw_lock:
        raise CapabilityRejected("state-root-unavailable")
    root = Path(raw_root).resolve(strict=True)
    lock = Path(raw_lock)
    metadata = root.stat()
    if (
        not stat.S_ISDIR(metadata.st_mode)
        or metadata.st_uid != os.getuid()
        or stat.S_IMODE(metadata.st_mode) != 0o700
    ):
        raise CapabilityRejected("state-root-unsafe")
    capabilities = root / "capabilities"
    if not capabilities.exists():
        capabilities.mkdir(mode=0o700)
        fsync_directory(root)
    capability_metadata = capabilities.stat()
    if (
        not stat.S_ISDIR(capability_metadata.st_mode)
        or capability_metadata.st_uid != os.getuid()
        or stat.S_IMODE(capability_metadata.st_mode) != 0o700
    ):
        raise CapabilityRejected("capability-root-unsafe")
    return root, lock, capabilities


def open_lease(lock: Path) -> int:
    flags = os.O_RDWR | os.O_CLOEXEC
    if hasattr(os, "O_NOFOLLOW"):
        flags |= os.O_NOFOLLOW
    try:
        descriptor = os.open(lock, flags)
    except OSError as error:
        raise CapabilityRejected("lease-open-failed") from error
    try:
        opened = os.fstat(descriptor)
        named = os.stat(lock, follow_symlinks=False)
        if (
            not stat.S_ISREG(opened.st_mode)
            or opened.st_uid != os.getuid()
            or stat.S_IMODE(opened.st_mode) != 0o600
            or opened.st_dev != named.st_dev
            or opened.st_ino != named.st_ino
        ):
            raise CapabilityRejected("lease-identity-mismatch")
        try:
            fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError as error:
            raise CapabilityRejected("lease-busy") from error
        if os.get_inheritable(descriptor):
            raise CapabilityRejected("lease-fd-inheritable")
        return descriptor
    except BaseException:
        os.close(descriptor)
        raise


def close_lease(descriptor: int) -> None:
    try:
        fcntl.flock(descriptor, fcntl.LOCK_UN)
    finally:
        os.close(descriptor)


def validate_lease_cloexec(descriptor: int) -> None:
    if os.get_inheritable(descriptor):
        raise CapabilityRejected("lease-fd-inheritable")


def issue_capability(
    claims: dict[str, str | int],
    *,
    deadline_ns: int | None = None,
) -> str:
    _, lock, capabilities = state_paths()
    descriptor = open_lease(lock)
    try:
        capability_id = secrets.token_hex(16)
        secret = secrets.token_hex(32)
        record = {
            "contract": CONTRACT,
            "capability_id": capability_id,
            "verifier": digest_text(secret),
            "phase": "issued",
            "ownership": "provisional",
            "expires_at_monotonic_ns": (
                time.monotonic_ns() + CAPABILITY_TTL_NS
                if deadline_ns is None
                else deadline_ns
            ),
            "claims": claims,
        }
        write_json_atomic(capabilities / f"{capability_id}.json", record)
        return f"{capability_id}.{secret}"
    finally:
        close_lease(descriptor)


def parse_capability(token: str) -> tuple[str, str]:
    capability_id, separator, secret = token.partition(".")
    if (
        not separator
        or len(capability_id) != 32
        or len(secret) != 64
        or any(
            character not in "0123456789abcdef" for character in token.replace(".", "")
        )
    ):
        raise CapabilityRejected("capability-malformed")
    return capability_id, secret


def consume_capability(
    token: str,
    expected_claims: dict[str, str | int],
) -> tuple[int, dict[str, Any]]:
    capability_id, secret = parse_capability(token)
    _, lock, capabilities = state_paths()
    descriptor = open_lease(lock)
    try:
        path = capabilities / f"{capability_id}.json"
        try:
            record = read_json_bounded(path)
        except FileNotFoundError as error:
            raise CapabilityRejected("capability-unknown") from error
        if record.get("contract") != CONTRACT:
            raise CapabilityRejected("capability-contract-mismatch")
        verifier = record.get("verifier")
        if not isinstance(verifier, str) or not secrets.compare_digest(
            verifier, digest_text(secret)
        ):
            raise CapabilityRejected("token-verifier-mismatch")
        if record.get("phase") != "issued" or record.get("ownership") != "provisional":
            raise CapabilityRejected("capability-not-issued")
        deadline = record.get("expires_at_monotonic_ns")
        if not isinstance(deadline, int) or time.monotonic_ns() >= deadline:
            raise CapabilityRejected("capability-expired")
        if record.get("claims") != expected_claims:
            raise CapabilityRejected("claim-mismatch")
        record["phase"] = "consumed"
        record["ownership"] = "runtime_owned"
        write_json_atomic(path, record)
        return descriptor, record
    except BaseException:
        close_lease(descriptor)
        raise


def split_provider_args(arguments: list[str]) -> tuple[list[str], tuple[str, ...]]:
    if "--" not in arguments:
        raise CapabilityRejected("provider-argv-delimiter-missing")
    index = arguments.index("--")
    return arguments[:index], tuple(arguments[index + 1 :])


def internal_prepare(arguments: list[str]) -> int:
    control, provider_args = split_provider_args(arguments)
    parser = argparse.ArgumentParser(add_help=False)
    parser.add_argument("--provider", required=True)
    parser.add_argument("--leader-pid", required=True, type=int)
    options = parser.parse_args(control)
    if os.getppid() != options.leader_pid:
        raise CapabilityRejected("prepare-not-direct-shell-child")
    claims = build_claims(options.provider, provider_args, options.leader_pid)
    token = issue_capability(claims)
    write_json_atomic(
        Path(os.environ["WSNAV_PREPARE_RECORD"]),
        {
            "broker_pid": os.getpid(),
            "broker_ppid": os.getppid(),
            "leader_pid": options.leader_pid,
            "provider": options.provider,
            "argv_digest": digest_argv(provider_args),
            "token_output_only": True,
        },
    )
    print(token)
    return 0


def internal_helper(arguments: list[str]) -> int:
    control, provider_args = split_provider_args(arguments)
    parser = argparse.ArgumentParser(add_help=False)
    parser.add_argument("--provider", required=True)
    parser.add_argument("--capability", required=True)
    options = parser.parse_args(control)
    claims = build_claims(options.provider, provider_args, os.getpid())
    descriptor, _ = consume_capability(options.capability, claims)
    try:
        validate_lease_cloexec(descriptor)
        identity = process_identity(os.getpid())
        write_json_atomic(
            Path(os.environ["WSNAV_HELPER_RECORD"]),
            {
                "pid": os.getpid(),
                "birth": identity["birth"],
                "pgid": identity["pgid"],
                "sid": identity["sid"],
                "provider": options.provider,
                "argv_digest": digest_argv(provider_args),
                "lease_fd_cloexec": not os.get_inheritable(descriptor),
                "phase": "runtime_owned_launching",
            },
        )
        environment = dict(os.environ)
        executable = sys.executable
        provider_argv = [
            executable,
            str(Path(__file__).resolve()),
            "--internal",
            "provider",
            "--provider",
            options.provider,
            "--",
            *provider_args,
        ]
        os.execve(executable, provider_argv, environment)
    except BaseException:
        close_lease(descriptor)
        raise


def lease_fd_inherited(lock: Path) -> bool:
    expected = lock.stat()
    descriptor_root = Path("/proc/self/fd")
    for entry in descriptor_root.iterdir():
        try:
            candidate = entry.stat()
        except OSError:
            continue
        if candidate.st_dev == expected.st_dev and candidate.st_ino == expected.st_ino:
            return True
    return False


def internal_provider(arguments: list[str]) -> int:
    control, provider_args = split_provider_args(arguments)
    parser = argparse.ArgumentParser(add_help=False)
    parser.add_argument("--provider", required=True)
    options = parser.parse_args(control)
    identity = process_identity(os.getpid())
    write_json_atomic(
        Path(os.environ["WSNAV_PROVIDER_RECORD"]),
        {
            "pid": os.getpid(),
            "ppid": identity["ppid"],
            "pgid": identity["pgid"],
            "sid": identity["sid"],
            "birth": identity["birth"],
            "cwd": str(Path.cwd().resolve(strict=True)),
            "provider": options.provider,
            "provider_args": list(provider_args),
            "lease_fd_inherited": lease_fd_inherited(
                Path(os.environ["WSNAV_LOCK_PATH"])
            ),
        },
    )
    print(OUTPUT_MARKER, flush=True)
    signal.signal(signal.SIGTERM, lambda _signum, _frame: sys.exit(0))
    try:
        while True:
            time.sleep(0.2)
    except KeyboardInterrupt:
        return 0


def wait_until(predicate: Any, reason: str) -> None:
    deadline = time.monotonic() + WAIT_TIMEOUT_SECONDS
    while time.monotonic() < deadline:
        if predicate():
            return
        time.sleep(POLL_SECONDS)
    raise SpikeFailure(reason)


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
    if not value.isdigit() or int(value) <= 0:
        raise SpikeFailure("pane-pid-malformed")
    return int(value)


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


def send_interrupt(socket: Path, configuration: Path, session: str) -> None:
    private_tmux(
        socket,
        configuration,
        "send-keys",
        "-t",
        f"{session}:0.0",
        "C-c",
        check=False,
    )


def kill_server(socket: Path, configuration: Path) -> None:
    private_tmux(socket, configuration, "kill-server", check=False)


def setup_state(root: Path) -> tuple[Path, Path]:
    state = root / "state"
    state.mkdir(mode=0o700)
    lock = state / "provisional.lock"
    descriptor = os.open(
        lock,
        os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_CLOEXEC,
        0o600,
    )
    try:
        os.write(descriptor, b"version=1\n")
        os.fsync(descriptor)
    finally:
        os.close(descriptor)
    fsync_directory(state)
    return state, lock


def synthetic_context(case_root: Path) -> dict[str, str]:
    return {
        "candidate_runtime_id": str(uuid.uuid4()),
        "lease_generation": secrets.token_hex(16),
        "presentation_id": str(uuid.uuid4()),
        "presentation_revision": "11",
        "registry_revision": "17",
        "request_id": str(uuid.uuid4()),
        "runtime_generation": secrets.token_hex(16),
        "runtime_paths_digest": digest_text(str(case_root / "runtime")),
        "slot_generation": secrets.token_hex(16),
    }


def capability_records(state: Path) -> list[dict[str, Any]]:
    directory = state / "capabilities"
    if not directory.is_dir():
        return []
    return [read_json_bounded(path) for path in sorted(directory.glob("*.json"))]


def run_shell_case(
    root: Path,
    configuration: Path,
    repo_child: Path,
    shell: str,
    provider: str,
    bashrc: Path,
    zsh_directory: Path,
) -> dict[str, bool | str]:
    case_name = f"{shell}_{provider}"
    case_root = root / case_name
    case_root.mkdir(mode=0o700)
    state, lock = setup_state(case_root)
    socket = root / f"{case_name}.sock"
    prepare_record = case_root / "prepare.json"
    helper_record = case_root / "helper.json"
    provider_record = case_root / "provider.json"
    environment = dict(os.environ)
    environment.update(
        {
            "WSNAV_CONTEXT": json.dumps(
                synthetic_context(case_root), separators=(",", ":")
            ),
            "WSNAV_HELPER_RECORD": str(helper_record),
            "WSNAV_LOCK_PATH": str(lock),
            "WSNAV_PREPARE_RECORD": str(prepare_record),
            "WSNAV_PROVIDER_RECORD": str(provider_record),
            "WSNAV_PYTHON": sys.executable,
            "WSNAV_SPIKE": str(Path(__file__).resolve()),
            "WSNAV_STATE_ROOT": str(state),
        }
    )
    if shell == "bash":
        command = ["bash", "--noprofile", "--rcfile", str(bashrc), "-i"]
    elif shell == "zsh":
        environment["ZDOTDIR"] = str(zsh_directory)
        command = ["zsh", "-d", "-i"]
    else:
        raise SpikeFailure("shell-unsupported")
    private_tmux(
        socket,
        configuration,
        "new-session",
        "-d",
        "-s",
        case_name,
        "-n",
        "shell",
        "-c",
        str(repo_child),
        shlex.join(command),
        environment=environment,
    )
    initial_pid = pane_pid(socket, configuration, case_name)
    initial = process_identity(initial_pid)
    command_line = shlex.join([provider, *EXPECTED_PROVIDER_ARGS])
    send_line(socket, configuration, case_name, command_line)
    wait_until(provider_record.is_file, "provider-record-timeout")
    prepared = read_json_bounded(prepare_record)
    helper = read_json_bounded(helper_record)
    observed = read_json_bounded(provider_record)
    records = capability_records(state)
    if len(records) != 1:
        raise SpikeFailure("capability-record-count-invalid")
    capability = records[0]
    claims = capability.get("claims")
    if not isinstance(claims, dict):
        raise SpikeFailure("capability-claims-malformed")
    provider_pid = int(observed["pid"])
    assertions: dict[str, bool | str] = {
        "shell": shell,
        "provider": provider,
        "prepare_is_direct_shell_child": prepared.get("broker_ppid") == initial_pid,
        "prepare_pid_differs_from_shell": prepared.get("broker_pid") != initial_pid,
        "prepare_returns_token_only": prepared.get("token_output_only") is True,
        "capability_persists_verifier_not_token": isinstance(
            capability.get("verifier"), str
        )
        and len(str(capability.get("verifier"))) == 64,
        "all_claims_bound": set(claims) == set(CLAIM_KEYS),
        "provider_and_args_bound": claims.get("provider") == provider
        and claims.get("argv_digest") == digest_argv(EXPECTED_PROVIDER_ARGS),
        "cwd_and_git_root_bound": claims.get("cwd_digest")
        == digest_text(str(repo_child.resolve(strict=True)))
        and claims.get("root_digest")
        == digest_text(str(repo_child.parent.resolve(strict=True))),
        "capability_consumed_once": capability.get("phase") == "consumed",
        "runtime_owned_before_provider_exec": capability.get("ownership")
        == "runtime_owned"
        and helper.get("phase") == "runtime_owned_launching",
        "helper_preserves_shell_pid": helper.get("pid") == initial_pid,
        "helper_preserves_shell_birth": helper.get("birth") == initial["birth"],
        "provider_preserves_shell_pid": provider_pid == initial_pid,
        "provider_preserves_shell_birth": observed.get("birth") == initial["birth"],
        "provider_preserves_process_group": observed.get("pgid") == initial["pgid"]
        and observed.get("pgid") == provider_pid,
        "provider_preserves_session": observed.get("sid") == initial["sid"],
        "provider_args_preserved": tuple(observed.get("provider_args", []))
        == EXPECTED_PROVIDER_ARGS,
        "provider_cwd_preserved": observed.get("cwd")
        == str(repo_child.resolve(strict=True)),
        "lease_fd_marked_cloexec": helper.get("lease_fd_cloexec") is True,
        "lease_fd_not_inherited": observed.get("lease_fd_inherited") is False,
    }
    if not all(
        value for key, value in assertions.items() if key not in {"shell", "provider"}
    ):
        raise SpikeFailure(f"{case_name}-assertion-failed")
    send_interrupt(socket, configuration, case_name)
    wait_until(lambda: not process_alive(provider_pid), "provider-did-not-exit")
    kill_server(socket, configuration)
    return assertions


def rejection_reason(call: Any) -> str:
    try:
        call()
    except CapabilityRejected as error:
        return str(error)
    raise SpikeFailure("expected-capability-rejection-missing")


def close_consumed(value: tuple[int, dict[str, Any]]) -> None:
    close_lease(value[0])


def run_fail_closed_matrix(root: Path) -> dict[str, bool]:
    model_root = root / "fail_closed"
    model_root.mkdir(mode=0o700)
    state, lock = setup_state(model_root)
    old_state = os.environ.get("WSNAV_STATE_ROOT")
    old_lock = os.environ.get("WSNAV_LOCK_PATH")
    os.environ["WSNAV_STATE_ROOT"] = str(state)
    os.environ["WSNAV_LOCK_PATH"] = str(lock)
    base_claims: dict[str, str | int] = {key: f"synthetic-{key}" for key in CLAIM_KEYS}
    base_claims.update(
        {
            "leader_pid": 101,
            "leader_pgid": 101,
            "leader_sid": 99,
            "presentation_revision": "11",
            "registry_revision": "17",
            "provider": "codex",
        }
    )
    try:
        replay_token = issue_capability(base_claims)
        close_consumed(consume_capability(replay_token, base_claims))
        replay_rejected = (
            rejection_reason(lambda: consume_capability(replay_token, base_claims))
            == "capability-not-issued"
        )

        expired_token = issue_capability(
            base_claims, deadline_ns=time.monotonic_ns() - 1
        )
        expired_rejected = (
            rejection_reason(lambda: consume_capability(expired_token, base_claims))
            == "capability-expired"
        )

        verifier_token = issue_capability(base_claims)
        capability_id, secret = parse_capability(verifier_token)
        replacement = "0" if secret[-1] != "0" else "1"
        invalid_token = f"{capability_id}.{secret[:-1]}{replacement}"
        verifier_rejected = (
            rejection_reason(lambda: consume_capability(invalid_token, base_claims))
            == "token-verifier-mismatch"
        )
        close_consumed(consume_capability(verifier_token, base_claims))

        all_mutations_rejected = True
        mismatches_leave_issued = True
        for key in CLAIM_KEYS:
            token = issue_capability(base_claims)
            changed = dict(base_claims)
            if isinstance(changed[key], int):
                changed[key] = int(changed[key]) + 1
            else:
                changed[key] = f"{changed[key]}-changed"
            all_mutations_rejected = all_mutations_rejected and (
                rejection_reason(
                    lambda token=token, changed=changed: consume_capability(
                        token, changed
                    )
                )
                == "claim-mismatch"
            )
            record_id, _ = parse_capability(token)
            record = read_json_bounded(state / "capabilities" / f"{record_id}.json")
            mismatches_leave_issued = mismatches_leave_issued and (
                record.get("phase") == "issued"
                and record.get("ownership") == "provisional"
            )
            close_consumed(consume_capability(token, base_claims))

        before_grammar = len(capability_records(state))
        grammar_rejected = (
            rejection_reason(
                lambda: approve_provider_argv("codex", ("resume", "thread-id"))
            )
            == "grammar-rejected"
        )
        grammar_created_no_capability = len(capability_records(state)) == before_grammar

        descriptor = open_lease(lock)
        os.set_inheritable(descriptor, True)
        inheritable_rejected = (
            rejection_reason(lambda: validate_lease_cloexec(descriptor))
            == "lease-fd-inheritable"
        )
        os.set_inheritable(descriptor, False)
        close_lease(descriptor)
    finally:
        if old_state is None:
            os.environ.pop("WSNAV_STATE_ROOT", None)
        else:
            os.environ["WSNAV_STATE_ROOT"] = old_state
        if old_lock is None:
            os.environ.pop("WSNAV_LOCK_PATH", None)
        else:
            os.environ["WSNAV_LOCK_PATH"] = old_lock
    return {
        "replay_rejected": replay_rejected,
        "expired_capability_rejected": expired_rejected,
        "invalid_verifier_rejected_without_consuming": verifier_rejected,
        "every_bound_claim_mutation_rejected": all_mutations_rejected,
        "claim_mismatch_does_not_consume": mismatches_leave_issued,
        "reserved_grammar_rejected_before_issue": grammar_rejected
        and grammar_created_no_capability,
        "inheritable_lease_fd_rejected": inheritable_rejected,
    }


def sanitized_result(
    *,
    status: str,
    reason: str,
    versions: dict[str, str],
    cases: dict[str, dict[str, bool | str]],
    fail_closed: dict[str, bool],
    assertions: dict[str, bool],
) -> dict[str, Any]:
    return {
        "study": STUDY,
        "contract_fingerprint": CONTRACT,
        "status": status,
        "reason": reason,
        "environment": versions,
        "cases": cases,
        "fail_closed": fail_closed,
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


def harness(arguments: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--result", type=Path)
    options = parser.parse_args(arguments)
    root: Path | None = None
    sockets: list[Path] = []
    configuration: Path | None = None
    before_fingerprint = ""
    baseline_established = False
    versions: dict[str, str] = {}
    cases: dict[str, dict[str, bool | str]] = {}
    fail_closed: dict[str, bool] = {}
    assertions: dict[str, bool] = {}
    status = "pass"
    reason = "two_phase_boundary_observed"
    try:
        required = ("bash", "git", "tmux", "zsh")
        if any(shutil.which(command) is None for command in required):
            raise SpikeBlocked("required-command-unavailable")
        versions = {
            "bash_version": tool_version("bash", "--version"),
            "tmux_version": tool_version("tmux", "-V"),
            "zsh_version": tool_version("zsh", "--version"),
        }
        before_fingerprint = ordinary_tmux_fingerprint()
        baseline_established = True
        root = Path(tempfile.mkdtemp(prefix=ROOT_PREFIX))
        root.chmod(0o700)
        assertions["temporary_root_mode_0700"] = (
            stat.S_IMODE(root.stat().st_mode) == 0o700
        )
        configuration = root / "tmux.conf"
        write_tmux_config(configuration)
        bashrc = root / "bashrc"
        write_private(bashrc, WRAPPER_SOURCE, 0o600)
        zsh_directory = root / "zsh"
        zsh_directory.mkdir(mode=0o700)
        write_private(zsh_directory / ".zshrc", WRAPPER_SOURCE, 0o600)
        repo = root / "repo"
        repo.mkdir(mode=0o700)
        initialized = run(["git", "-C", str(repo), "init", "-q"])
        if initialized.returncode != 0:
            raise SpikeBlocked("disposable-git-init-failed")
        repo_child = repo / "nested"
        repo_child.mkdir(mode=0o700)

        for shell in ("bash", "zsh"):
            for provider in PROVIDERS:
                case_name = f"{shell}_{provider}"
                sockets.append(root / f"{case_name}.sock")
                cases[case_name] = run_shell_case(
                    root,
                    configuration,
                    repo_child,
                    shell,
                    provider,
                    bashrc,
                    zsh_directory,
                )
        fail_closed = run_fail_closed_matrix(root)
        assertions.update(
            {
                "bash_and_zsh_observed": {case["shell"] for case in cases.values()}
                == {"bash", "zsh"},
                "codex_and_opencode_routes_observed": {
                    case["provider"] for case in cases.values()
                }
                == {"codex", "opencode"},
                "all_shell_case_assertions_pass": all(
                    value
                    for case in cases.values()
                    for key, value in case.items()
                    if key not in {"shell", "provider"}
                ),
                "all_fail_closed_assertions_pass": all(fail_closed.values()),
                "private_tmux_only": True,
            }
        )
        if not all(assertions.values()):
            raise SpikeFailure("aggregate-assertion-failed")
    except SpikeBlocked as error:
        status = "blocked"
        reason = str(error)
    except (CapabilityRejected, SpikeFailure) as error:
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
        assertions["cleanup_complete"] = root is None or not root.exists()
        if baseline_established:
            after_fingerprint = ordinary_tmux_fingerprint()
            assertions["ordinary_tmux_unchanged"] = (
                before_fingerprint == after_fingerprint
            )
        else:
            assertions["ordinary_tmux_unchanged"] = False
        if not assertions["cleanup_complete"]:
            status = "falsified"
            reason = "temporary-root-cleanup-failed"
        if baseline_established and not assertions["ordinary_tmux_unchanged"]:
            status = "falsified"
            reason = "ordinary-tmux-changed"
    result = sanitized_result(
        status=status,
        reason=reason,
        versions=versions,
        cases=cases,
        fail_closed=fail_closed,
        assertions=assertions,
    )
    write_result(options.result, result)
    return 0 if status == "pass" else 1


def main() -> int:
    arguments = sys.argv[1:]
    if len(arguments) >= 2 and arguments[:2] == ["--internal", "prepare"]:
        try:
            return internal_prepare(arguments[2:])
        except CapabilityRejected as error:
            print(f"wsnav-spike: {error}", file=sys.stderr)
            return 64
    if len(arguments) >= 2 and arguments[:2] == ["--internal", "helper"]:
        try:
            return internal_helper(arguments[2:])
        except CapabilityRejected as error:
            print(f"wsnav-spike: {error}", file=sys.stderr)
            return 64
    if len(arguments) >= 2 and arguments[:2] == ["--internal", "provider"]:
        return internal_provider(arguments[2:])
    return harness(arguments)


if __name__ == "__main__":
    raise SystemExit(main())
