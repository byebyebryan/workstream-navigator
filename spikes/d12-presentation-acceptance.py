#!/usr/bin/env python3
"""Bounded local slice of the D12 operator-gated presentation acceptance.

The ``--confirm-live-d12`` path is the explicitly authorized local workflow;
it leaves visual completed-output assertions for operator confirmation after
machine cleanup.  The result format is aggregate-only: it contains
no paths, identities, process numbers, terminal text, credentials, or SSH
diagnostics.
"""

from __future__ import annotations

import argparse
import base64
import binascii
import contextlib
import fcntl
import getpass
import hashlib
import json
import os
import pty
import re
import select
import shlex
import shutil
import signal
import socket
import sqlite3
import stat
import struct
import subprocess
import sys
import tempfile
import termios
import threading
import time
import uuid
from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass, replace
from pathlib import Path
from typing import Any

STUDY = "d12-presentation-acceptance"
RESULT_SCHEMA = 2
MAX_REASON_LENGTH = 96
MAX_TOPOLOGY_BYTES = 16 * 1024
MAX_RESULT_BYTES = 16 * 1024
ROOT_MARKER = ".wsnav-d12-owned"
ROOT_MARKER_CONTENT = "d12-acceptance-root-v1\n"

ALLOWED_STATUSES = frozenset({"blocked", "falsified", "pass"})
ALLOWED_REASONS = frozenset(
    {
        "candidate-binary-missing",
        "candidate-not-current-checkout",
        "attachment-attempt-invalid",
        "attachment-host-invalid",
        "attachment-not-running",
        "attachment-parse-invalid",
        "attachment-workstream-duplicate",
        "attachment-workstream-unknown",
        "cleanup-incomplete",
        "cleanup-process-survived",
        "internal-error",
        "management-command-failed",
        "not-implemented",
        "operator-confirmation-required",
        "operator-declined",
        "operator-terminal-unavailable",
        "ownership-ambiguous",
        "presentation-unavailable",
        "presentation-handle-invalid",
        "presentation-client-ambiguous",
        "presentation-client-invalid",
        "presentation-client-not-ready",
        "process-identity-ambiguous",
        "provider-auth-unavailable",
        "provider-unavailable",
        "provider-version-unsupported",
        "privacy-invalid",
        "visual-confirmation-required",
        "remote-checkout-registration-failed",
        "remote-host-registration-failed",
        "remote-park-failed",
        "remote-start-failed",
        "self-test-failed",
        "sentinel-missing",
        "ssh-daemon-unavailable",
        "ssh-key-unavailable",
        "ssh-remote-unavailable",
        "ssh-setup-failed",
        "timeout",
        "topology-invalid",
        "topology-blank-geometry",
        "topology-dead-pane",
        "topology-geometry-invalid",
        "topology-context-invalid",
        "topology-parse-invalid",
        "topology-role-missing",
        "ordinary-tmux-changed",
        "runtime-not-live",
        "utility-command-changed",
        "utility-context-changed",
        "utility-context-invalid",
        "utility-cwd-changed",
        "utility-cwd-invalid",
        "utility-metadata-invalid",
        "utility-pane-changed",
        "utility-process-changed",
        "utility-process-invalid",
        "utility-role-missing",
    }
)
ALLOWED_CLEANUP_STATUSES = frozenset({"complete", "incomplete", "not-run"})
ALLOWED_CLEANUP_REASONS = frozenset(
    {
        "complete",
        "not-attempted",
        "cleanup-incomplete",
        "cleanup-process-survived",
        "internal-error",
        "management-command-failed",
        "ordinary-tmux-changed",
        "ownership-ambiguous",
        "presentation-handle-invalid",
        "presentation-unavailable",
        "process-identity-ambiguous",
        "remote-park-failed",
        "timeout",
    }
)
ALLOWED_TOOL_STATES = frozenset({"not-run", "checked"})

ASSERTION_NAMES = (
    "abi2_preflight",
    "cleanup_complete",
    "local_below_provider_geometry",
    "local_canonical_cwd",
    "local_completed_output_preserved",
    "local_detach_reattach",
    "local_git_status",
    "local_guarded_close",
    "local_one_shell_idempotent",
    "local_provider_interactive",
    "local_running_attachment",
    "local_runtime_identity",
    "local_shell_exit_cleanup",
    "local_topology",
    "local_utility_context_fixed",
    "ordinary_tmux_unchanged",
    "privacy_bounded",
    "ssh_below_provider_geometry",
    "ssh_canonical_cwd",
    "ssh_completed_output_preserved",
    "ssh_detach_reattach",
    "ssh_git_status",
    "ssh_guarded_close",
    "ssh_one_shell_idempotent",
    "ssh_provider_interactive",
    "ssh_running_attachment",
    "ssh_runtime_identity",
    "ssh_shell_exit_cleanup",
    "ssh_topology",
    "ssh_utility_context_fixed",
)

COMMON_MACHINE_ASSERTIONS = frozenset(
    {
        "abi2_preflight",
        "cleanup_complete",
        "ordinary_tmux_unchanged",
        "privacy_bounded",
    }
)
LOCAL_MACHINE_ASSERTIONS = COMMON_MACHINE_ASSERTIONS | frozenset(
    {
        "local_below_provider_geometry",
        "local_canonical_cwd",
        "local_detach_reattach",
        "local_git_status",
        "local_guarded_close",
        "local_one_shell_idempotent",
        "local_provider_interactive",
        "local_running_attachment",
        "local_runtime_identity",
        "local_shell_exit_cleanup",
        "local_topology",
        "local_utility_context_fixed",
    }
)
REMOTE_MACHINE_ASSERTIONS = COMMON_MACHINE_ASSERTIONS | frozenset(
    {
        "ssh_below_provider_geometry",
        "ssh_canonical_cwd",
        "ssh_detach_reattach",
        "ssh_git_status",
        "ssh_guarded_close",
        "ssh_one_shell_idempotent",
        "ssh_provider_interactive",
        "ssh_running_attachment",
        "ssh_runtime_identity",
        "ssh_shell_exit_cleanup",
        "ssh_topology",
        "ssh_utility_context_fixed",
    }
)

_PANE_ID = re.compile(r"^%[0-9]+$")
_WORKSTREAM_ID = re.compile(
    r"^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[1-5][0-9a-fA-F]{3}-[89abAB][0-9a-fA-F]{3}-[0-9a-fA-F]{12}$"
)
_UUID_TEXT = re.compile(
    r"^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$"
)
PRESENTATION_PANE_FORMAT = (
    "#{pane_id}\t#{@wsnav_role}\t#{@wsnav_host_alias}\t"
    "#{@wsnav_workstream_id}\t#{pane_dead}\t#{pane_left}\t#{pane_top}\t"
    "#{pane_width}\t#{pane_height}\t#{window_width}\t#{window_height}"
)
UTILITY_METADATA_FORMAT = (
    "#{pane_id}\t#{pane_pid}\t#{pane_start_command}\t#{pane_current_command}\t"
    "#{pane_current_path}\t#{@wsnav_host_alias}\t#{@wsnav_workstream_id}"
)
PRESENTATION_CLIENT_FORMAT = "#{client_session}"
RUNTIME_METADATA_FORMAT = (
    "#{pane_id}\t#{pane_pid}\t#{pane_dead}\t#{pane_start_command}\t#{pane_current_path}"
)
MAX_COMMAND_OUTPUT = 16 * 1024
COMMAND_TIMEOUT_SECONDS = 60
PRESENTATION_WAIT_SECONDS = 30
OPERATOR_ATTACH_SECONDS = 900
POLL_SECONDS = 0.1
AUTH_RELATIVE_PATH = Path("opencode") / "auth.json"
NONRETRYABLE_WAIT_REASONS = frozenset(
    {
        "ownership-ambiguous",
        "presentation-handle-invalid",
        "presentation-unavailable",
        "topology-blank-geometry",
        "topology-dead-pane",
        "topology-geometry-invalid",
        "topology-parse-invalid",
        "topology-role-missing",
    }
)
INITIAL_TOPOLOGY_RETRY_REASONS = frozenset(
    {
        "presentation-unavailable",
        "topology-blank-geometry",
        "topology-dead-pane",
        "topology-parse-invalid",
        "topology-role-missing",
    }
)


class HarnessBlocked(RuntimeError):
    """A bounded reason that may be exposed in the result."""

    def __init__(self, reason: str) -> None:
        if reason not in ALLOWED_REASONS:
            reason = "internal-error"
        super().__init__(reason)
        self.reason = reason


class ResultPrivacyError(ValueError):
    """The aggregate result attempted to retain disallowed evidence."""


@dataclass(frozen=True)
class Options:
    confirm_live: bool
    result: Path | None
    self_test: bool


@dataclass(frozen=True)
class PaneRecord:
    """Only disposable tmux metadata; never emitted in evidence."""

    pane_id: str
    role: str
    host_alias: str
    workstream_id: str
    dead: bool
    left: int
    top: int
    width: int
    height: int
    window_width: int
    window_height: int

    @property
    def right(self) -> int:
        return self.left + self.width

    @property
    def bottom(self) -> int:
        return self.top + self.height


@dataclass(frozen=True)
class DisposableRoot:
    path: Path


@dataclass(frozen=True)
class PresentationHandle:
    socket: Path
    session: str
    directory: Path


@dataclass(frozen=True)
class UtilityMetadata:
    pane_id: str
    pane_pid: int
    process_birth: str
    start_command: str
    current_command: str
    current_path: Path
    host_alias: str
    workstream_id: str


@dataclass(frozen=True)
class RuntimeIdentity:
    socket: Path
    session: str
    pane_id: str
    pane_pid: int
    process_birth: str
    start_command: str
    current_path: Path


@dataclass(frozen=True)
class AttachmentEvidence:
    """Validated ephemeral attachment identity; never emitted in evidence."""

    attempt_id: str
    host_alias: str
    workstream_id: str
    phase: str


@dataclass
class NavigatorProcess:
    process: subprocess.Popen[bytes]
    master_fd: int
    drain_thread: threading.Thread
    stop_event: threading.Event
    process_birth: str


@dataclass(frozen=True)
class LocalFixture:
    root: DisposableRoot
    state_root: Path
    provider_root: Path
    provider_env: dict[str, str]
    project_roots: tuple[Path, Path]
    workstream_ids: tuple[str, str]
    sentinel_paths: tuple[dict[str, Path], dict[str, Path]]


@dataclass
class SshMaterial:
    ssh_root: Path
    port: int
    username: str
    destination: str
    host_key: Path
    client_key: Path
    authorized_keys: Path
    known_hosts: Path
    sshd_config: Path
    client_config: Path
    client_home: Path
    client_wrapper: Path


@dataclass
class SshDaemon:
    process: subprocess.Popen[bytes]
    process_birth: str


@dataclass
class RemoteFixture:
    root: DisposableRoot
    client_state_root: Path
    client_env: dict[str, str]
    remote_state_root: Path
    remote_provider_root: Path
    remote_env: dict[str, str]
    project_roots: tuple[Path, Path]
    workstream_ids: tuple[str, str]
    sentinel_paths: tuple[dict[str, Path], dict[str, Path]]
    host_alias: str
    destination: str
    remote_executable: Path
    ssh_material: SshMaterial
    sshd: SshDaemon


@dataclass(frozen=True)
class LocalWorkflowState:
    """The bounded local workflow checkpoints used by the live path.

    This state is intentionally internal.  It keeps the operator workflow
    monotonic and gives cleanup one explicit terminal boundary without ever
    putting terminal text or process identity into the result artifact.
    """

    phase: str


LOCAL_PHASES = (
    "setup",
    "first-attach",
    "provider-switch",
    "shell-exit",
    "guarded-close",
    "cleanup",
)


def _advance_local_phase(
    state: LocalWorkflowState, next_phase: str
) -> LocalWorkflowState:
    if next_phase not in LOCAL_PHASES:
        raise HarnessBlocked("internal-error")
    try:
        current_index = LOCAL_PHASES.index(state.phase)
        next_index = LOCAL_PHASES.index(next_phase)
    except ValueError as error:
        raise HarnessBlocked("internal-error") from error
    if next_index != current_index + 1:
        raise HarnessBlocked("internal-error")
    return LocalWorkflowState(next_phase)


def _begin_local_cleanup(state: LocalWorkflowState) -> LocalWorkflowState:
    if state.phase not in LOCAL_PHASES or state.phase == "cleanup":
        if state.phase == "cleanup":
            return state
        raise HarnessBlocked("internal-error")
    return LocalWorkflowState("cleanup")


def parse_arguments(argv: Sequence[str] | None = None) -> Options:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--confirm-live-d12", action="store_true")
    parser.add_argument("--result", type=Path)
    parser.add_argument("--self-test", action="store_true", help=argparse.SUPPRESS)
    arguments = parser.parse_args(argv)
    return Options(arguments.confirm_live_d12, arguments.result, arguments.self_test)


def default_assertions() -> dict[str, bool]:
    return {name: False for name in ASSERTION_NAMES}


def default_tool_versions() -> dict[str, str]:
    return {name: "not-run" for name in ("ssh", "tmux", "wsnav")}


def make_result(
    *,
    status: str,
    reason: str,
    operator_confirmed: bool,
    primary_status: str | None = None,
    primary_reason: str | None = None,
    assertions: Mapping[str, bool] | None = None,
    tool_versions: Mapping[str, str] | None = None,
    cleanup_status: str = "not-run",
    cleanup_reason: str = "not-attempted",
) -> dict[str, Any]:
    result = {
        "schema": RESULT_SCHEMA,
        "study": STUDY,
        "status": status,
        "reason": reason,
        "primary_status": status if primary_status is None else primary_status,
        "primary_reason": reason if primary_reason is None else primary_reason,
        "operator_confirmed": operator_confirmed,
        "assertions": dict(default_assertions() if assertions is None else assertions),
        "tool_versions": dict(
            default_tool_versions() if tool_versions is None else tool_versions
        ),
        "cleanup": {"status": cleanup_status, "reason": cleanup_reason},
    }
    validate_result_privacy(result)
    return result


def validate_result_privacy(result: Mapping[str, Any]) -> None:
    """Reject paths, identities, process numbers, and terminal evidence."""

    expected = {
        "schema",
        "study",
        "status",
        "reason",
        "primary_status",
        "primary_reason",
        "operator_confirmed",
        "assertions",
        "tool_versions",
        "cleanup",
    }
    if set(result) != expected or result.get("schema") != RESULT_SCHEMA:
        raise ResultPrivacyError("schema")
    if result.get("study") != STUDY:
        raise ResultPrivacyError("study")
    if result.get("status") not in ALLOWED_STATUSES:
        raise ResultPrivacyError("status")
    if result.get("reason") not in ALLOWED_REASONS:
        raise ResultPrivacyError("reason")
    if result.get("primary_status") not in ALLOWED_STATUSES:
        raise ResultPrivacyError("primary-status")
    if result.get("primary_reason") not in ALLOWED_REASONS:
        raise ResultPrivacyError("primary-reason")
    if type(result.get("operator_confirmed")) is not bool:
        raise ResultPrivacyError("operator-confirmed")

    assertions = result.get("assertions")
    if not isinstance(assertions, Mapping) or set(assertions) != set(ASSERTION_NAMES):
        raise ResultPrivacyError("assertions")
    if any(type(value) is not bool for value in assertions.values()):
        raise ResultPrivacyError("assertion-value")

    versions = result.get("tool_versions")
    if not isinstance(versions, Mapping) or set(versions) != {"ssh", "tmux", "wsnav"}:
        raise ResultPrivacyError("tool-versions")
    for value in versions.values():
        if value not in ALLOWED_TOOL_STATES:
            raise ResultPrivacyError("tool-version")

    cleanup = result.get("cleanup")
    if not isinstance(cleanup, Mapping) or set(cleanup) != {"status", "reason"}:
        raise ResultPrivacyError("cleanup")
    if cleanup.get("status") not in ALLOWED_CLEANUP_STATUSES:
        raise ResultPrivacyError("cleanup-status")
    if cleanup.get("reason") not in ALLOWED_CLEANUP_REASONS:
        raise ResultPrivacyError("cleanup-reason")

    encoded = json.dumps(result, separators=(",", ":"), sort_keys=True).encode()
    if len(encoded) > MAX_RESULT_BYTES:
        raise ResultPrivacyError("result-size")
    text = encoded.decode()
    if any(character in text for character in ("/", "\\", "\n", "\r")):
        raise ResultPrivacyError("path-or-terminal-text")
    forbidden_keys = (
        "path",
        "pid",
        "hostname",
        "socket",
        "prompt",
        "response",
        "credential",
    )
    if any(
        any(fragment in key.lower() for fragment in forbidden_keys) for key in result
    ):
        raise ResultPrivacyError("forbidden-key")


def write_result(path: Path, result: Mapping[str, Any]) -> None:
    validate_result_privacy(result)
    path.parent.mkdir(parents=True, exist_ok=True)
    encoded = (json.dumps(result, indent=2, sort_keys=True) + "\n").encode()
    if len(encoded) > MAX_RESULT_BYTES:
        raise ResultPrivacyError("result-size")
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    try:
        os.fchmod(descriptor, 0o600)
        with os.fdopen(descriptor, "wb") as output:
            descriptor = -1
            output.write(encoded)
    finally:
        if descriptor != -1:
            os.close(descriptor)


def resolve_candidate_binary(workspace: Path) -> Path:
    """Resolve only ``target/debug/wsnav`` from this checkout."""

    checkout = workspace.resolve()
    candidate = checkout / "target" / "debug" / "wsnav"
    if not candidate.is_file() or not os.access(candidate, os.X_OK):
        raise HarnessBlocked("candidate-binary-missing")
    resolved = candidate.resolve(strict=True)
    expected = (checkout / "target" / "debug").resolve()
    if resolved.parent != expected or resolved.name != "wsnav":
        raise HarnessBlocked("candidate-not-current-checkout")
    return resolved


def ordinary_tmux_fingerprint(
    runner: Callable[
        [Sequence[str], Mapping[str, str], float], subprocess.CompletedProcess[str]
    ]
    | None = None,
) -> str:
    """Hash a read-only ordinary-server listing without returning inventory."""

    environment = os.environ.copy()
    environment.pop("TMUX", None)
    arguments = (
        "tmux",
        "list-sessions",
        "-F",
        "#{session_name}:#{session_created}:#{session_windows}",
        "-O",
        "name",
    )
    if runner is None:
        try:
            completed = subprocess.run(
                arguments,
                env=environment,
                stdout=subprocess.PIPE,
                stderr=subprocess.DEVNULL,
                text=True,
                timeout=3,
                check=False,
            )
        except (OSError, subprocess.TimeoutExpired):
            return "unavailable"
    else:
        completed = runner(arguments, environment, 3.0)
    if completed.returncode != 0:
        return "absent"
    return hashlib.sha256(completed.stdout.encode()).hexdigest()


def create_disposable_root(prefix: str = "wsnav-d12.") -> DisposableRoot:
    path = Path(tempfile.mkdtemp(prefix=prefix))
    os.chmod(path, 0o700)
    if stat.S_IMODE(path.stat().st_mode) != 0o700:
        shutil.rmtree(path, ignore_errors=False)
        raise HarnessBlocked("ownership-ambiguous")
    marker = path / ROOT_MARKER
    marker.write_text(ROOT_MARKER_CONTENT, encoding="ascii")
    os.chmod(marker, 0o600)
    return DisposableRoot(path)


def _root_marker_valid(root: DisposableRoot) -> bool:
    try:
        path = root.path
        marker = path / ROOT_MARKER
        return (
            path.is_dir()
            and not path.is_symlink()
            and marker.is_file()
            and not marker.is_symlink()
            and marker.read_text(encoding="ascii") == ROOT_MARKER_CONTENT
        )
    except (OSError, UnicodeError):
        return False


def cleanup_disposable_root(root: DisposableRoot) -> str:
    """Remove only a root carrying the exact harness ownership marker."""

    path = root.path
    try:
        if not _root_marker_valid(root):
            return "ownership-ambiguous"
        shutil.rmtree(path)
        return "complete" if not path.exists() else "cleanup-incomplete"
    except (OSError, UnicodeError):
        return "cleanup-incomplete"


def parse_topology(text: str) -> tuple[PaneRecord, ...]:
    """Parse production list-panes metadata without retaining raw output."""

    try:
        return _parse_topology(text)
    except HarnessBlocked as error:
        if error.reason == "topology-invalid":
            raise HarnessBlocked("topology-parse-invalid") from error
        raise


def _parse_topology(text: str) -> tuple[PaneRecord, ...]:

    if len(text.encode()) > MAX_TOPOLOGY_BYTES:
        raise HarnessBlocked("topology-invalid")
    records: list[PaneRecord] = []
    for line in text.splitlines():
        fields = line.split("\t")
        if len(fields) != 11:
            raise HarnessBlocked("topology-invalid")
        pane_id, role, host, workstream, dead, *numbers = fields
        if not _PANE_ID.fullmatch(pane_id) or role not in {
            "navigator",
            "provider",
            "utility",
        }:
            raise HarnessBlocked("topology-invalid")
        if any(
            any(character.isspace() for character in value)
            for value in (host, workstream)
        ):
            raise HarnessBlocked("topology-invalid")
        if dead not in {"0", "1"}:
            raise HarnessBlocked("topology-invalid")
        try:
            values = tuple(int(value) for value in numbers)
        except ValueError as error:
            raise HarnessBlocked("topology-invalid") from error
        if len(values) != 6 or any(value < 0 for value in values):
            raise HarnessBlocked("topology-invalid")
        left, top, width, height, window_width, window_height = values
        if not width or not height or not window_width or not window_height:
            raise HarnessBlocked("topology-invalid")
        records.append(
            PaneRecord(
                pane_id,
                role,
                host,
                workstream,
                dead == "1",
                left,
                top,
                width,
                height,
                window_width,
                window_height,
            )
        )
    if not records or len({record.pane_id for record in records}) != len(records):
        raise HarnessBlocked("topology-invalid")
    return tuple(records)


def _touches(left: int, right: int) -> bool:
    return abs(left - right) <= 1


def validate_supported_topology(records: Sequence[PaneRecord]) -> None:
    """Validate the two supported geometries, allowing one border cell."""

    if len(records) not in {2, 3} or any(record.dead for record in records):
        raise HarnessBlocked("topology-invalid")
    if len({(record.window_width, record.window_height) for record in records}) != 1:
        raise HarnessBlocked("topology-invalid")
    navigator = [record for record in records if record.role == "navigator"]
    providers = [record for record in records if record.role == "provider"]
    utilities = [record for record in records if record.role == "utility"]
    if len(navigator) != 1 or len(providers) != 1 or len(utilities) != len(records) - 2:
        raise HarnessBlocked("topology-invalid")
    navigator = navigator[0]
    provider = providers[0]
    window_height = navigator.window_height
    if navigator.left > 1 or navigator.top > 1 or navigator.bottom < window_height - 1:
        raise HarnessBlocked("topology-invalid")
    if provider.top > 1 or (provider.bottom < window_height - 1 and not utilities):
        raise HarnessBlocked("topology-invalid")
    if not _touches(navigator.right, provider.left):
        raise HarnessBlocked("topology-invalid")
    if not provider.host_alias or not _WORKSTREAM_ID.fullmatch(provider.workstream_id):
        raise HarnessBlocked("topology-context-invalid")
    if not utilities:
        return
    utility = utilities[0]
    if (
        not utility.host_alias
        or not _WORKSTREAM_ID.fullmatch(utility.workstream_id)
        or utility.left != provider.left
        or utility.width != provider.width
        or not _touches(provider.bottom, utility.top)
        or utility.bottom < window_height - 1
    ):
        if not utility.host_alias or not _WORKSTREAM_ID.fullmatch(
            utility.workstream_id
        ):
            raise HarnessBlocked("topology-context-invalid")
        raise HarnessBlocked("topology-invalid")


def _run_command(
    arguments: Sequence[str | os.PathLike[str]],
    *,
    env: Mapping[str, str] | None = None,
    cwd: Path | None = None,
    timeout: float = COMMAND_TIMEOUT_SECONDS,
) -> subprocess.CompletedProcess[str]:
    """Run bounded management/setup output, never a provider or shell pane."""

    try:
        return subprocess.run(
            [os.fspath(argument) for argument in arguments],
            cwd=cwd,
            env=None if env is None else dict(env),
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            text=True,
            timeout=timeout,
            check=False,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        raise HarnessBlocked(
            "timeout"
            if isinstance(error, subprocess.TimeoutExpired)
            else "management-command-failed"
        ) from error


def _wsnav_command(
    candidate: Path,
    state_root: Path,
    arguments: Sequence[str],
    *,
    env: Mapping[str, str],
    cwd: Path | None = None,
) -> str:
    result = _run_command(
        [candidate, "--state-root", state_root, *arguments], env=env, cwd=cwd
    )
    if result.returncode != 0:
        raise HarnessBlocked("management-command-failed")
    if len(result.stdout.encode()) > MAX_COMMAND_OUTPUT:
        raise HarnessBlocked("management-command-failed")
    return result.stdout


def _source_opencode_auth() -> Path:
    source_data = os.environ.get("XDG_DATA_HOME")
    data_home = Path(source_data) if source_data else Path.home() / ".local" / "share"
    return data_home / AUTH_RELATIVE_PATH


def _opencode_environment(provider_root: Path) -> dict[str, str]:
    environment = os.environ.copy()
    for variable, suffix in (
        ("XDG_CONFIG_HOME", "xdg-config"),
        ("XDG_DATA_HOME", "xdg-data"),
        ("XDG_CACHE_HOME", "xdg-cache"),
        ("XDG_STATE_HOME", "xdg-state"),
    ):
        directory = provider_root / suffix
        directory.mkdir(parents=True, exist_ok=True)
        os.chmod(directory, 0o700)
        environment[variable] = str(directory)
    for variable in ("OPENCODE_CONFIG", "OPENCODE_CONFIG_DIR", "OPENCODE_DATA_DIR"):
        environment.pop(variable, None)
    environment["HISTFILE"] = os.devnull
    environment["SAVEHIST"] = "0"
    source = _source_opencode_auth()
    try:
        source_stat = source.lstat()
    except OSError as error:
        raise HarnessBlocked("provider-auth-unavailable") from error
    if not stat.S_ISREG(source_stat.st_mode):
        raise HarnessBlocked("provider-auth-unavailable")
    target = Path(environment["XDG_DATA_HOME"]) / AUTH_RELATIVE_PATH
    target.parent.mkdir(parents=True, exist_ok=True)
    os.chmod(target.parent, 0o700)
    # This is intentionally the sole ordinary-provider file copied by the
    # live harness. Its bytes are never parsed, logged, or returned.
    shutil.copyfile(source, target)
    os.chmod(target, stat.S_IMODE(source_stat.st_mode))
    return environment


def _disposable_client_environment(root: Path) -> dict[str, str]:
    """Create navigator-only XDG state without copying ordinary credentials."""

    environment = os.environ.copy()
    for variable, suffix in (
        ("XDG_CONFIG_HOME", "xdg-config"),
        ("XDG_DATA_HOME", "xdg-data"),
        ("XDG_CACHE_HOME", "xdg-cache"),
        ("XDG_STATE_HOME", "xdg-state"),
    ):
        directory = root / suffix
        directory.mkdir(parents=True, exist_ok=True)
        os.chmod(directory, 0o700)
        environment[variable] = str(directory)
    for variable in ("OPENCODE_CONFIG", "OPENCODE_CONFIG_DIR", "OPENCODE_DATA_DIR"):
        environment.pop(variable, None)
    environment.pop("SSH_AUTH_SOCK", None)
    environment.pop("TMUX", None)
    environment["HISTFILE"] = os.devnull
    environment["SAVEHIST"] = "0"
    return environment


def _free_loopback_port() -> int:
    try:
        with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as listener:
            listener.bind(("127.0.0.1", 0))
            port = int(listener.getsockname()[1])
    except OSError as error:
        raise HarnessBlocked("ssh-setup-failed") from error
    if not 1024 < port < 65536:
        raise HarnessBlocked("ssh-setup-failed")
    return port


def _safe_ssh_username() -> str:
    username = getpass.getuser()
    if (
        not username
        or username.startswith("-")
        or not re.fullmatch(r"[A-Za-z0-9._-]+", username)
    ):
        raise HarnessBlocked("ssh-setup-failed")
    return username


def _ssh_material(
    root: Path, *, port: int, username: str, destination: str
) -> SshMaterial:
    if not root.is_absolute() or not 1024 < port < 65536:
        raise HarnessBlocked("ssh-setup-failed")
    if username.startswith("-") or not re.fullmatch(r"[A-Za-z0-9._-]+", username):
        raise HarnessBlocked("ssh-setup-failed")
    if destination.startswith("-") or not re.fullmatch(r"[A-Za-z0-9._-]+", destination):
        raise HarnessBlocked("ssh-setup-failed")
    ssh_root = root / "ssh"
    client_home = ssh_root / "client-home"
    client_config_root = client_home / ".ssh"
    client_bin = client_home / "bin"
    return SshMaterial(
        ssh_root,
        port,
        username,
        destination,
        ssh_root / "host_ed25519",
        ssh_root / "client_ed25519",
        ssh_root / "authorized_keys",
        client_config_root / "known_hosts",
        ssh_root / "sshd_config",
        client_config_root / "config",
        client_home,
        client_bin / "ssh",
    )


def _sshd_config_text(material: SshMaterial) -> str:
    return (
        "\n".join(
            (
                f"Port {material.port}",
                "ListenAddress 127.0.0.1",
                f"HostKey {material.host_key}",
                f"PidFile {material.ssh_root / 'sshd.pid'}",
                f"AuthorizedKeysFile {material.authorized_keys}",
                "PubkeyAuthentication yes",
                "PasswordAuthentication no",
                "KbdInteractiveAuthentication no",
                "ChallengeResponseAuthentication no",
                "UsePAM no",
                "PermitRootLogin no",
                f"AllowUsers {material.username}",
                "StrictModes no",
                "AllowTcpForwarding no",
                "X11Forwarding no",
                "PermitTunnel no",
                "UseDNS no",
                "LogLevel QUIET",
            )
        )
        + "\n"
    )


def _ssh_client_config_text(material: SshMaterial) -> str:
    return (
        "\n".join(
            (
                f"Host {material.destination}",
                "HostName 127.0.0.1",
                f"Port {material.port}",
                f"User {material.username}",
                f"IdentityFile {material.client_key}",
                "IdentitiesOnly yes",
                "IdentityAgent none",
                "BatchMode yes",
                "StrictHostKeyChecking yes",
                "GlobalKnownHostsFile none",
                f"HostKeyAlias {material.destination}",
                f"UserKnownHostsFile {material.known_hosts}",
                "LogLevel QUIET",
            )
        )
        + "\n"
    )


def _ssh_client_wrapper_text(material: SshMaterial, ssh_binary: Path) -> str:
    if not ssh_binary.is_absolute():
        raise HarnessBlocked("ssh-setup-failed")
    return (
        "#!/bin/sh\n"
        "set -eu\n"
        f'exec {shlex.quote(str(ssh_binary))} -F {shlex.quote(str(material.client_config))} "$@"\n'
    )


def _remote_wrapper_text(
    candidate: Path, state_root: Path, provider_environment: Mapping[str, str]
) -> str:
    if not candidate.is_absolute() or not state_root.is_absolute():
        raise HarnessBlocked("ssh-setup-failed")
    exports = []
    for name in (
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "XDG_CACHE_HOME",
        "XDG_STATE_HOME",
    ):
        value = provider_environment.get(name)
        if (
            value is None
            or not Path(value).is_absolute()
            or any(character in value for character in "\r\n")
        ):
            raise HarnessBlocked("ssh-setup-failed")
        exports.append(f"export {name}={shlex.quote(value)}")
    path = provider_environment.get("PATH")
    if path is None or not path or any(character in path for character in "\r\n"):
        raise HarnessBlocked("ssh-setup-failed")
    exports.append(f"export PATH={shlex.quote(path)}")
    exports.extend(("export HISTFILE=/dev/null", "export SAVEHIST=0"))
    return (
        "#!/bin/sh\n"
        "set -eu\n"
        + "\n".join(exports)
        + "\n"
        + f'exec {shlex.quote(str(candidate.resolve()))} --state-root {shlex.quote(str(state_root.resolve()))} "$@"\n'
    )


def _write_executable(path: Path, content: str) -> None:
    try:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content, encoding="utf-8")
        path.chmod(0o700)
    except OSError as error:
        raise HarnessBlocked("ssh-setup-failed") from error


def _known_hosts_from_keyscan(material: SshMaterial, output: str) -> str:
    """Bind scanner evidence to the configured disposable host alias."""

    if len(output.encode()) > MAX_COMMAND_OUTPUT:
        raise HarnessBlocked("ssh-daemon-unavailable")
    expected_hosts = {"127.0.0.1", f"[127.0.0.1]:{material.port}"}
    entries: list[str] = []
    for line in output.splitlines():
        stripped = line.strip()
        if not stripped or stripped.startswith("#"):
            continue
        fields = stripped.split()
        if len(fields) != 3 or fields[0] not in expected_hosts:
            raise HarnessBlocked("ssh-daemon-unavailable")
        if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._-]*", fields[1]):
            raise HarnessBlocked("ssh-daemon-unavailable")
        try:
            base64.b64decode(fields[2], validate=True)
        except (ValueError, binascii.Error) as error:
            raise HarnessBlocked("ssh-daemon-unavailable") from error
        entries.append(f"{material.destination} {fields[1]} {fields[2]}")
    unique_entries = tuple(dict.fromkeys(entries))
    if not unique_entries:
        raise HarnessBlocked("ssh-daemon-unavailable")
    return "\n".join(unique_entries) + "\n"


def _prepare_ssh_material(material: SshMaterial) -> None:
    ssh_keygen = shutil.which("ssh-keygen")
    ssh_binary = shutil.which("ssh")
    if ssh_keygen is None or ssh_binary is None:
        raise HarnessBlocked("ssh-key-unavailable")
    try:
        material.ssh_root.mkdir(parents=True, exist_ok=False)
        material.client_home.mkdir(mode=0o700)
        material.client_config.parent.mkdir(mode=0o700)
        material.client_wrapper.parent.mkdir(mode=0o700)
        os.chmod(material.ssh_root, 0o700)
        host_key = _run_command(
            [ssh_keygen, "-q", "-t", "ed25519", "-N", "", "-f", material.host_key],
            timeout=15,
        )
        client_key = _run_command(
            [ssh_keygen, "-q", "-t", "ed25519", "-N", "", "-f", material.client_key],
            timeout=15,
        )
        if (
            host_key.returncode != 0
            or client_key.returncode != 0
            or not material.host_key.is_file()
            or not material.client_key.is_file()
            or not material.client_key.with_suffix(".pub").is_file()
        ):
            raise HarnessBlocked("ssh-key-unavailable")
        public_key = material.client_key.with_suffix(".pub").read_text(encoding="ascii")
        material.authorized_keys.write_text(public_key, encoding="ascii")
        material.authorized_keys.chmod(0o600)
        material.known_hosts.touch(mode=0o600)
        material.known_hosts.chmod(0o600)
        material.sshd_config.write_text(_sshd_config_text(material), encoding="ascii")
        material.sshd_config.chmod(0o600)
        material.client_config.write_text(
            _ssh_client_config_text(material), encoding="ascii"
        )
        material.client_config.chmod(0o600)
        _write_executable(
            material.client_wrapper,
            _ssh_client_wrapper_text(material, Path(ssh_binary)),
        )
    except (OSError, UnicodeError) as error:
        raise HarnessBlocked("ssh-key-unavailable") from error


def _ssh_client_environment(
    material: SshMaterial, base: Mapping[str, str] | None = None
) -> dict[str, str]:
    environment = dict(os.environ if base is None else base)
    environment.pop("SSH_AUTH_SOCK", None)
    environment.pop("TMUX", None)
    path = environment.get("PATH", "")
    environment["PATH"] = f"{material.client_wrapper.parent}{os.pathsep}{path}"
    return environment


def _wait_for_sshd(process: subprocess.Popen[bytes], port: int) -> None:
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise HarnessBlocked("ssh-daemon-unavailable")
        try:
            with socket.create_connection(("127.0.0.1", port), timeout=0.25):
                return
        except OSError:
            time.sleep(POLL_SECONDS)
    raise HarnessBlocked("ssh-daemon-unavailable")


def _start_disposable_sshd(
    root: Path,
) -> tuple[SshMaterial, SshDaemon, dict[str, str]]:
    sshd = shutil.which("sshd")
    ssh_keyscan = shutil.which("ssh-keyscan")
    if sshd is None or ssh_keyscan is None:
        raise HarnessBlocked("ssh-daemon-unavailable")
    material = _ssh_material(
        root.resolve(),
        port=_free_loopback_port(),
        username=_safe_ssh_username(),
        destination="wsnav-d12-loopback",
    )
    _prepare_ssh_material(material)
    try:
        process = subprocess.Popen(
            [sshd, "-D", "-e", "-f", str(material.sshd_config)],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            start_new_session=True,
        )
    except OSError as error:
        raise HarnessBlocked("ssh-daemon-unavailable") from error
    birth = _process_birth(process.pid)
    if birth is None:
        with contextlib.suppress(OSError):
            os.killpg(process.pid, signal.SIGTERM)
        raise HarnessBlocked("process-identity-ambiguous")
    daemon = SshDaemon(process, birth)
    try:
        _wait_for_sshd(process, material.port)
        keyscan = _run_command(
            [ssh_keyscan, "-T", "5", "-p", str(material.port), "127.0.0.1"],
            timeout=10,
        )
        if keyscan.returncode != 0 or not keyscan.stdout.strip():
            raise HarnessBlocked("ssh-daemon-unavailable")
        material.known_hosts.write_text(
            _known_hosts_from_keyscan(material, keyscan.stdout), encoding="ascii"
        )
        material.known_hosts.chmod(0o600)
    except HarnessBlocked:
        _stop_disposable_sshd(daemon)
        raise
    except (OSError, UnicodeError) as error:
        _stop_disposable_sshd(daemon)
        raise HarnessBlocked("ssh-daemon-unavailable") from error
    return material, daemon, _ssh_client_environment(material)


def _stop_disposable_sshd(daemon: SshDaemon) -> None:
    process = daemon.process
    current = _process_birth(process.pid)
    if current not in {None, daemon.process_birth}:
        raise HarnessBlocked("process-identity-ambiguous")
    if current is None:
        return
    try:
        os.killpg(process.pid, signal.SIGTERM)
        process.wait(timeout=5)
    except ProcessLookupError:
        return
    except subprocess.TimeoutExpired as error:
        if _process_birth(process.pid) != daemon.process_birth:
            raise HarnessBlocked("process-identity-ambiguous") from error
        os.killpg(process.pid, signal.SIGKILL)
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired as final_error:
            raise HarnessBlocked("cleanup-process-survived") from final_error
    except OSError as error:
        raise HarnessBlocked("cleanup-incomplete") from error


def _remote_probe_arguments(
    material: SshMaterial, remote_executable: Path
) -> tuple[str, ...]:
    return (
        "ssh",
        material.destination,
        str(remote_executable),
        "_probe",
    )


def _probe_remote_abi(
    candidate: Path,
    material: SshMaterial,
    remote_executable: Path,
    client_environment: Mapping[str, str],
    runner: Callable[..., subprocess.CompletedProcess[str]] | None = None,
) -> None:
    if not candidate.is_absolute() or not remote_executable.is_absolute():
        raise HarnessBlocked("ssh-setup-failed")
    arguments = _remote_probe_arguments(material, remote_executable)
    if runner is None:
        result = _run_command(arguments, env=client_environment, timeout=15)
    else:
        result = runner(arguments, env=client_environment, timeout=15)
    if result.returncode != 0 or len(result.stdout.encode()) > MAX_COMMAND_OUTPUT:
        raise HarnessBlocked("ssh-remote-unavailable")
    try:
        value = json.loads(result.stdout)
    except json.JSONDecodeError as error:
        raise HarnessBlocked("ssh-remote-unavailable") from error
    if not isinstance(value, dict) or value.get("control_abi") != 2:
        raise HarnessBlocked("candidate-not-current-checkout")
    if value.get("protocol_version") != 18 or value.get("host_schema_version") != 12:
        raise HarnessBlocked("candidate-not-current-checkout")


def _create_repository(path: Path) -> None:
    path.mkdir(parents=True, exist_ok=False)
    _run_command(["git", "init", "-q", "-b", "main", path])
    _run_command(["git", "-C", path, "config", "user.name", "wsnav-d12"])
    _run_command(["git", "-C", path, "config", "user.email", "wsnav-d12@example.test"])
    (path / "README").write_text("disposable\n", encoding="ascii")
    _run_command(["git", "-C", path, "add", "README"])
    _run_command(["git", "-C", path, "commit", "-qm", "initial"])


def _create_sentinels(project: Path) -> dict[str, Path]:
    paths = {
        name: project / f".wsnav-d12-{name}.ok" for name in ("hostname", "pwd", "git")
    }
    script = project / ".wsnav-d12-check"
    expected = str(project.resolve())
    script.write_text(
        "#!/bin/sh\n"
        "set -eu\n"
        'case "${1:-}" in\n'
        f"hostname) hostname >/dev/null 2>&1 && : > {paths['hostname']!s} ;;\n"
        f'pwd) test "$(pwd -P)" = {expected} && : > {paths["pwd"]!s} ;;\n'
        f"git) git -C {expected} status --porcelain >/dev/null 2>&1 && : > {paths['git']!s} ;;\n"
        "*) exit 2 ;;\n"
        "esac\n",
        encoding="utf-8",
    )
    script.chmod(0o700)
    return paths


def _probe_candidate_abi(candidate: Path) -> None:
    result = _run_command([candidate, "_probe"])
    if result.returncode != 0 or len(result.stdout.encode()) > MAX_COMMAND_OUTPUT:
        raise HarnessBlocked("candidate-not-current-checkout")
    try:
        value = json.loads(result.stdout)
    except json.JSONDecodeError as error:
        raise HarnessBlocked("candidate-not-current-checkout") from error
    if not isinstance(value, dict):
        raise HarnessBlocked("candidate-not-current-checkout")
    if (
        value.get("control_abi") != 2
        or value.get("protocol_version") != 18
        or value.get("host_schema_version") != 12
    ):
        raise HarnessBlocked("candidate-not-current-checkout")


def _probe_opencode(environment: Mapping[str, str]) -> None:
    result = _run_command(["opencode", "--version"], env=environment, timeout=10)
    if result.returncode != 0:
        raise HarnessBlocked("provider-unavailable")
    version = result.stdout.strip()
    if not re.search(r"(?:^|\s)1\.18(?:\.\d+)?(?:\s|$)", version):
        raise HarnessBlocked("provider-version-unsupported")


def _extract_workstream_id(output: str) -> str:
    for token in reversed(output.split()):
        if _WORKSTREAM_ID.fullmatch(token):
            return str(uuid.UUID(token))
    raise HarnessBlocked("management-command-failed")


def _read_workstream_revision(state_root: Path, workstream_id: str) -> int:
    database = state_root / "host.sqlite"
    try:
        with sqlite3.connect(database) as connection:
            row = connection.execute(
                "SELECT revision FROM workstreams WHERE workstream_id = ?",
                (workstream_id,),
            ).fetchone()
    except sqlite3.Error as error:
        raise HarnessBlocked("management-command-failed") from error
    if row is None or type(row[0]) is not int or row[0] <= 0:
        raise HarnessBlocked("management-command-failed")
    return row[0]


def _remote_register_arguments(
    host_alias: str, destination: str, executable: Path
) -> tuple[str, ...]:
    return (
        "register-remote",
        host_alias,
        "--destination",
        destination,
        "--executable",
        str(executable),
    )


def _remote_checkout_arguments(host_alias: str, project_root: Path) -> tuple[str, ...]:
    return (
        "host",
        "register-checkout",
        host_alias,
        str(project_root),
        "--provider",
        "opencode",
    )


def _remote_start_arguments(
    host_alias: str, workstream_id: str, revision: int
) -> tuple[str, ...]:
    return ("host", "start", host_alias, workstream_id, str(revision))


def _remote_park_arguments(
    host_alias: str, workstream_id: str, revision: int
) -> tuple[str, ...]:
    return ("host", "park", host_alias, workstream_id, str(revision))


def _remote_management_command(
    candidate: Path,
    state_root: Path,
    arguments: Sequence[str],
    *,
    env: Mapping[str, str],
    failure_reason: str,
    runner: Callable[[Path, Path, Sequence[str], Mapping[str, str]], str] | None = None,
) -> str:
    """Run one bounded remote setup action with a stage-only failure category."""

    try:
        if runner is None:
            return _wsnav_command(candidate, state_root, arguments, env=env)
        return runner(candidate, state_root, arguments, env)
    except HarnessBlocked as error:
        if error.reason in {"management-command-failed", "timeout"}:
            raise HarnessBlocked(failure_reason) from error
        raise


def _read_workstream_ids(state_root: Path) -> tuple[str, ...]:
    database = state_root / "host.sqlite"
    if not database.exists():
        return ()
    try:
        with sqlite3.connect(database) as connection:
            rows = connection.execute(
                "SELECT workstream_id FROM workstreams"
            ).fetchall()
    except sqlite3.Error as error:
        raise HarnessBlocked("management-command-failed") from error
    values = tuple(row[0] for row in rows if isinstance(row[0], str))
    if len(values) != len(rows) or any(
        not _WORKSTREAM_ID.fullmatch(value) for value in values
    ):
        raise HarnessBlocked("management-command-failed")
    return values


def _build_local_fixture(candidate: Path, root: DisposableRoot) -> LocalFixture:
    provider_root = root.path / "opencode-home"
    provider_env = _opencode_environment(provider_root)
    _probe_opencode(provider_env)
    state_root = root.path / "state"
    project_a = root.path / "project-a"
    project_b = root.path / "project-b"
    _create_repository(project_a)
    _create_repository(project_b)
    sentinels = (_create_sentinels(project_a), _create_sentinels(project_b))
    registered_a = _wsnav_command(
        candidate,
        state_root,
        ["register", "--provider", "opencode", str(project_a)],
        env=provider_env,
    )
    workstream_a = _extract_workstream_id(registered_a)
    registered_b = _wsnav_command(
        candidate,
        state_root,
        ["register", "--provider", "opencode", str(project_b)],
        env=provider_env,
    )
    workstream_b = _extract_workstream_id(registered_b)
    _wsnav_command(candidate, state_root, ["start", workstream_a], env=provider_env)
    _wsnav_command(candidate, state_root, ["start", workstream_b], env=provider_env)
    return LocalFixture(
        root,
        state_root,
        provider_root,
        provider_env,
        (project_a, project_b),
        (workstream_a, workstream_b),
        sentinels,
    )


def _build_remote_fixture(candidate: Path, root: DisposableRoot) -> RemoteFixture:
    """Build the loopback host and client state without opening presentation UI."""

    client_state_root = root.path / "client-state"
    remote_state_root = root.path / "remote-state"
    remote_provider_root = root.path / "remote-opencode-home"
    client_env = _disposable_client_environment(root.path / "client-xdg")
    remote_env = _opencode_environment(remote_provider_root)
    _probe_opencode(remote_env)
    project_a = root.path / "remote-project-a"
    project_b = root.path / "remote-project-b"
    _create_repository(project_a)
    _create_repository(project_b)
    sentinels = (_create_sentinels(project_a), _create_sentinels(project_b))
    host_alias = "loopback-d12"
    material: SshMaterial | None = None
    daemon: SshDaemon | None = None
    client_ssh_environment: dict[str, str] | None = None
    workstream_ids: list[str] = []
    started_runtime_identities: list[RuntimeIdentity] = []
    remote_executable = root.path / "remote-bin" / "wsnav"
    try:
        material, daemon, client_ssh_environment = _start_disposable_sshd(root.path)
        client_ssh_environment = _ssh_client_environment(material, client_env)
        _write_executable(
            remote_executable,
            _remote_wrapper_text(candidate, remote_state_root, remote_env),
        )
        _probe_remote_abi(
            candidate.resolve(),
            material,
            remote_executable,
            client_ssh_environment,
        )
        _remote_management_command(
            candidate,
            client_state_root,
            _remote_register_arguments(
                host_alias, material.destination, remote_executable
            ),
            env=client_ssh_environment,
            failure_reason="remote-host-registration-failed",
        )
        for project in (project_a, project_b):
            output = _remote_management_command(
                candidate,
                client_state_root,
                _remote_checkout_arguments(host_alias, project),
                env=client_ssh_environment,
                failure_reason="remote-checkout-registration-failed",
            )
            workstream_ids.append(_extract_workstream_id(output))
        for workstream_id in workstream_ids:
            revision = _read_workstream_revision(remote_state_root, workstream_id)
            _remote_management_command(
                candidate,
                client_state_root,
                _remote_start_arguments(host_alias, workstream_id, revision),
                env=client_ssh_environment,
                failure_reason="remote-start-failed",
            )
            started_runtime_identities.append(
                _wait_until(
                    lambda workstream_id=workstream_id: _runtime_identity(
                        remote_state_root, workstream_id
                    ),
                    timeout=PRESENTATION_WAIT_SECONDS,
                    reason="runtime-not-live",
                )
            )
    except BaseException:
        if client_ssh_environment is not None:
            for workstream_id in workstream_ids:
                try:
                    revision = _read_workstream_revision(
                        remote_state_root, workstream_id
                    )
                    _remote_management_command(
                        candidate,
                        client_state_root,
                        _remote_park_arguments(host_alias, workstream_id, revision),
                        env=client_ssh_environment,
                        failure_reason="remote-park-failed",
                    )
                except (HarnessBlocked, OSError, sqlite3.Error, ValueError):
                    pass
            for identity in started_runtime_identities:
                try:
                    _wait_for_runtime_disappearance(identity)
                except (HarnessBlocked, OSError, sqlite3.Error, ValueError):
                    pass
        if daemon is not None:
            _stop_disposable_sshd(daemon)
        raise
    assert material is not None
    assert daemon is not None
    assert client_ssh_environment is not None
    return RemoteFixture(
        root,
        client_state_root,
        client_ssh_environment,
        remote_state_root,
        remote_provider_root,
        remote_env,
        (project_a, project_b),
        tuple(workstream_ids),
        sentinels,
        host_alias,
        material.destination,
        remote_executable,
        material,
        daemon,
    )


def _wait_until(
    predicate: Callable[[], Any], *, timeout: float, reason: str = "timeout"
) -> Any:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            value = predicate()
        except HarnessBlocked as error:
            if error.reason in NONRETRYABLE_WAIT_REASONS:
                raise
            value = None
        except (OSError, sqlite3.Error):
            value = None
        if value:
            return value
        time.sleep(POLL_SECONDS)
    raise HarnessBlocked(reason)


def _presentation_handle(state_root: Path) -> PresentationHandle | None:
    presentation = _presentation_artifact(state_root)
    if presentation is None or not presentation.socket.exists():
        return None
    state = _presentation_server_state(presentation)
    if state == "live":
        return presentation
    if state == "live-other":
        raise HarnessBlocked("presentation-handle-invalid")
    if state == "unknown":
        raise HarnessBlocked("ownership-ambiguous")
    return None


def _presentation_artifact(state_root: Path) -> PresentationHandle | None:
    """Find exactly one presentation directory, without requiring a live server."""

    state_root = state_root.resolve()
    directory = state_root / "presentation"
    if directory.is_symlink() or not directory.is_dir():
        return None
    candidates = sorted(
        entry
        for entry in directory.iterdir()
        if entry.is_dir()
        and not entry.is_symlink()
        and entry.name.startswith("presentation-")
    )
    if len(candidates) != 1:
        if not candidates:
            return None
        raise HarnessBlocked("presentation-handle-invalid")
    presentation = candidates[0]
    identifier = presentation.name.removeprefix("presentation-")
    if len(identifier) != 12 or not re.fullmatch(r"[0-9a-f]+", identifier):
        raise HarnessBlocked("presentation-handle-invalid")
    if presentation.parent != directory:
        raise HarnessBlocked("ownership-ambiguous")
    socket = presentation / "tmux.sock"
    if socket.is_symlink():
        raise HarnessBlocked("ownership-ambiguous")
    session = f"wsnav-presentation-{identifier}"
    return PresentationHandle(socket, session, presentation)


def _tmux_probe(
    socket: Path,
    arguments: Sequence[str],
    runner: Callable[[Sequence[str]], subprocess.CompletedProcess[str]] | None = None,
) -> subprocess.CompletedProcess[str]:
    command = ("tmux", "-S", str(socket), *arguments)
    if runner is not None:
        return runner(command)
    try:
        result = subprocess.run(
            command,
            capture_output=True,
            text=True,
            timeout=5,
            check=False,
        )
    except subprocess.TimeoutExpired as error:
        raise HarnessBlocked("timeout") from error
    except OSError as error:
        raise HarnessBlocked("management-command-failed") from error
    if (
        len(result.stdout.encode()) > MAX_COMMAND_OUTPUT
        or len(result.stderr.encode()) > MAX_COMMAND_OUTPUT
    ):
        raise HarnessBlocked("management-command-failed")
    return result


def _parse_presentation_client_count(output: str, session: str) -> int:
    clients = output.splitlines()
    if any(client != session for client in clients):
        raise HarnessBlocked("presentation-client-invalid")
    return len(clients)


def _presentation_client_count(
    handle: PresentationHandle,
    runner: Callable[[Sequence[str]], subprocess.CompletedProcess[str]] | None = None,
) -> int:
    result = _tmux_probe(
        handle.socket,
        [
            "list-clients",
            "-t",
            handle.session,
            "-F",
            PRESENTATION_CLIENT_FORMAT,
        ],
        runner,
    )
    if result.returncode != 0:
        diagnostic = result.stderr.lower()
        if "no clients" in diagnostic or "no client" in diagnostic:
            return 0
        raise HarnessBlocked("presentation-unavailable")
    return _parse_presentation_client_count(result.stdout, handle.session)


def _presentation_server_state(
    handle: PresentationHandle,
    runner: Callable[[Sequence[str]], subprocess.CompletedProcess[str]] | None = None,
) -> str:
    """Return live, live-other, or conclusively absent for the exact socket."""

    if not handle.socket.exists():
        return "absent"
    exact = _tmux_probe(
        handle.socket,
        ["has-session", "-t", handle.session],
        runner,
    )
    if exact.returncode == 0:
        return "live"
    any_session = _tmux_probe(handle.socket, ["list-sessions"], runner)
    if any_session.returncode == 0:
        return "live-other"
    diagnostic = any_session.stderr.lower()
    if "no server running" in diagnostic or "no such file or directory" in diagnostic:
        return "absent"
    return "unknown"


def _tmux_output(socket: Path, arguments: Sequence[str], *, timeout: float = 5) -> str:
    result = _run_command(["tmux", "-S", socket, *arguments], timeout=timeout)
    if result.returncode != 0 or len(result.stdout.encode()) > MAX_TOPOLOGY_BYTES:
        raise HarnessBlocked("presentation-unavailable")
    return result.stdout


def _validate_two_pane_shape(records: Sequence[PaneRecord]) -> None:
    if len(records) != 2:
        raise HarnessBlocked("topology-blank-geometry")
    if any(record.dead for record in records):
        raise HarnessBlocked("topology-dead-pane")
    navigator = [record for record in records if record.role == "navigator"]
    provider = [record for record in records if record.role == "provider"]
    if len(navigator) != 1 or len(provider) != 1:
        raise HarnessBlocked("topology-role-missing")
    navigator, provider = navigator[0], provider[0]
    if len({(record.window_width, record.window_height) for record in records}) != 1:
        raise HarnessBlocked("topology-blank-geometry")
    if (
        navigator.left > 1
        or navigator.top > 1
        or navigator.bottom < navigator.window_height - 1
        or provider.top > 1
        or provider.bottom < provider.window_height - 1
        or not _touches(navigator.right, provider.left)
    ):
        raise HarnessBlocked("topology-blank-geometry")


def _presentation_topology(
    handle: PresentationHandle, *, require_context: bool = True
) -> tuple[PaneRecord, ...]:
    output = _tmux_output(
        handle.socket,
        [
            "list-panes",
            "-t",
            f"{handle.session}:navigator",
            "-F",
            PRESENTATION_PANE_FORMAT,
        ],
    )
    records = parse_topology(output)
    if require_context:
        validate_supported_topology(records)
    else:
        _validate_two_pane_shape(records)
    return records


def _wait_for_initial_presentation(
    state_root: Path,
    *,
    handle_reader: Callable[[Path], PresentationHandle | None] = _presentation_handle,
    topology_reader: Callable[
        [PresentationHandle], tuple[PaneRecord, ...]
    ] = lambda handle: _presentation_topology(handle, require_context=False),
    timeout: float = PRESENTATION_WAIT_SECONDS,
    sleep: Callable[[float], None] = time.sleep,
) -> tuple[PresentationHandle, tuple[PaneRecord, ...]]:
    """Wait through the private server's bounded startup ordering.

    ``new-session`` makes the socket/session visible before role tags, the
    provider wait pane, and the final width hooks are all installed.  Those
    intermediate observations are retried without weakening the final exact
    two-pane geometry check; the last fixed category is retained on timeout.
    """

    deadline = time.monotonic() + timeout
    last_reason = "presentation-unavailable"
    while time.monotonic() < deadline:
        try:
            handle = handle_reader(state_root)
            if handle is not None:
                try:
                    records = topology_reader(handle)
                except HarnessBlocked as error:
                    if error.reason not in INITIAL_TOPOLOGY_RETRY_REASONS:
                        raise
                    last_reason = error.reason
                else:
                    return handle, records
        except HarnessBlocked as error:
            if error.reason not in INITIAL_TOPOLOGY_RETRY_REASONS:
                raise
            last_reason = error.reason
        sleep(POLL_SECONDS)
    raise HarnessBlocked(last_reason)


def _process_birth(pid: int) -> str | None:
    if pid <= 0:
        return None
    try:
        value = Path(f"/proc/{pid}/stat").read_text(encoding="ascii")
    except (FileNotFoundError, PermissionError, OSError):
        return None
    close = value.rfind(")")
    if close < 0:
        return None
    fields = value[close + 2 :].split()
    return fields[19] if len(fields) > 19 else None


def _utility_metadata(handle: PresentationHandle) -> UtilityMetadata:
    topology = _presentation_topology(handle)
    utility = next((record for record in topology if record.role == "utility"), None)
    if utility is None:
        raise HarnessBlocked("utility-role-missing")
    output = _tmux_output(
        handle.socket,
        [
            "display-message",
            "-p",
            "-t",
            utility.pane_id,
            UTILITY_METADATA_FORMAT,
        ],
    ).strip("\n")
    fields = output.split("\t")
    if len(fields) != 7 or fields[0] != utility.pane_id:
        raise HarnessBlocked("utility-metadata-invalid")
    try:
        pane_pid = int(fields[1])
    except ValueError as error:
        raise HarnessBlocked("utility-process-invalid") from error
    birth = _process_birth(pane_pid)
    if birth is None or any("\t" in value for value in fields[2:]):
        raise HarnessBlocked("utility-process-invalid")
    if not fields[4]:
        raise HarnessBlocked("utility-cwd-invalid")
    if not fields[5] or not fields[6]:
        raise HarnessBlocked("utility-context-invalid")
    return UtilityMetadata(
        fields[0],
        pane_pid,
        birth,
        fields[2],
        fields[3],
        Path(fields[4]),
        fields[5],
        fields[6],
    )


def _utility_metadata_difference(
    before: UtilityMetadata, after: UtilityMetadata
) -> str | None:
    """Return a fixed category for any forbidden utility-context mutation."""

    if before.pane_id != after.pane_id:
        return "utility-pane-changed"
    if (before.pane_pid, before.process_birth) != (
        after.pane_pid,
        after.process_birth,
    ):
        return "utility-process-changed"
    if (before.start_command, before.current_command) != (
        after.start_command,
        after.current_command,
    ):
        return "utility-command-changed"
    if before.current_path != after.current_path:
        return "utility-cwd-changed"
    if (before.host_alias, before.workstream_id) != (
        after.host_alias,
        after.workstream_id,
    ):
        return "utility-context-changed"
    return None


def _runtime_identity(state_root: Path, workstream_id: str) -> RuntimeIdentity:
    database = state_root / "host.sqlite"
    try:
        with sqlite3.connect(database) as connection:
            row = connection.execute(
                "SELECT runtime_id, tmux_session, cwd FROM runtimes "
                "WHERE workstream_id = ?",
                (workstream_id,),
            ).fetchone()
    except sqlite3.Error as error:
        raise HarnessBlocked("runtime-not-live") from error
    if row is None:
        raise HarnessBlocked("runtime-not-live")
    runtime_id, session, cwd = row
    if not all(
        isinstance(value, str) and value for value in (runtime_id, session, cwd)
    ):
        raise HarnessBlocked("runtime-not-live")
    socket = state_root / "run" / f"runtime-{runtime_id}" / "tmux.sock"
    output = _tmux_output(
        socket,
        ["list-panes", "-t", session, "-F", RUNTIME_METADATA_FORMAT],
    ).strip("\n")
    fields = output.split("\t")
    if len(fields) != 5 or fields[2] != "0":
        raise HarnessBlocked("runtime-not-live")
    try:
        pane_pid = int(fields[1])
    except ValueError as error:
        raise HarnessBlocked("runtime-not-live") from error
    birth = _process_birth(pane_pid)
    if birth is None:
        raise HarnessBlocked("runtime-not-live")
    return RuntimeIdentity(
        socket,
        session,
        fields[0],
        pane_pid,
        birth,
        fields[3],
        Path(fields[4]),
    )


def _read_attachment_status(handle: PresentationHandle) -> AttachmentEvidence:
    status_path = handle.directory / "attachment.json"
    try:
        encoded = status_path.read_bytes()
        if len(encoded) > 4096:
            raise HarnessBlocked("attachment-parse-invalid")
        value = json.loads(encoded)
    except FileNotFoundError as error:
        raise HarnessBlocked("attachment-not-running") from error
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        raise HarnessBlocked("attachment-parse-invalid") from error
    if not isinstance(value, dict):
        raise HarnessBlocked("attachment-parse-invalid")
    attempt_id = value.get("attempt_id")
    host_alias = value.get("host_alias")
    workstream_id = value.get("workstream_id")
    phase = value.get("phase")
    if not all(
        isinstance(field, str)
        for field in (attempt_id, host_alias, workstream_id, phase)
    ):
        raise HarnessBlocked("attachment-parse-invalid")
    if not _UUID_TEXT.fullmatch(attempt_id):
        raise HarnessBlocked("attachment-attempt-invalid")
    if not _WORKSTREAM_ID.fullmatch(workstream_id):
        raise HarnessBlocked("attachment-parse-invalid")
    if phase not in {"pending", "running", "completed", "failed"}:
        raise HarnessBlocked("attachment-parse-invalid")
    return AttachmentEvidence(
        str(uuid.UUID(attempt_id)),
        host_alias,
        str(uuid.UUID(workstream_id)),
        phase,
    )


def _select_running_attachment(
    handle: PresentationHandle,
    workstream_ids: Sequence[str],
    *,
    expected_host: str = "local",
) -> int:
    if len(set(workstream_ids)) != len(workstream_ids):
        raise HarnessBlocked("attachment-workstream-duplicate")
    evidence = _read_attachment_status(handle)
    if evidence.host_alias != expected_host:
        raise HarnessBlocked("attachment-host-invalid")
    if evidence.workstream_id not in workstream_ids:
        raise HarnessBlocked("attachment-workstream-unknown")
    if evidence.phase != "running":
        raise HarnessBlocked("attachment-not-running")
    matches = [
        index
        for index, value in enumerate(workstream_ids)
        if value == evidence.workstream_id
    ]
    if len(matches) != 1:
        raise HarnessBlocked("attachment-workstream-duplicate")
    return matches[0]


def _wait_for_running_attachment(
    handle: PresentationHandle,
    workstream_ids: Sequence[str],
    *,
    expected_host: str = "local",
    expected_index: int | None = None,
    timeout: float = PRESENTATION_WAIT_SECONDS,
    sleep: Callable[[float], None] = time.sleep,
) -> int:
    deadline = time.monotonic() + timeout
    last_reason = "attachment-not-running"
    retryable_reasons = frozenset(
        {
            "attachment-parse-invalid",
            "attachment-attempt-invalid",
            "attachment-not-running",
        }
    )
    while time.monotonic() < deadline:
        try:
            index = _select_running_attachment(
                handle, workstream_ids, expected_host=expected_host
            )
        except HarnessBlocked as error:
            if error.reason not in retryable_reasons:
                raise
            last_reason = error.reason
        else:
            if expected_index is None or index == expected_index:
                return index
            last_reason = "attachment-workstream-unknown"
        sleep(POLL_SECONDS)
    raise HarnessBlocked(last_reason)


def _attachment_status(
    handle: PresentationHandle,
    workstream_id: str,
    *,
    expected_host: str = "local",
    phase: str = "running",
) -> bool:
    try:
        evidence = _read_attachment_status(handle)
    except HarnessBlocked:
        return False
    return (
        evidence.host_alias == expected_host
        and evidence.workstream_id == workstream_id
        and evidence.phase == phase
    )


def _status_has_result_attention(
    candidate: Path, state_root: Path, workstream_id: str, env: Mapping[str, str]
) -> bool:
    output = _wsnav_command(candidate, state_root, ["status", workstream_id], env=env)
    return "private runtime: live" in output and "result attention: unseen" in output


def _remote_status_has_result_attention(
    material: SshMaterial,
    remote_executable: Path,
    workstream_id: str,
    env: Mapping[str, str],
    runner: Callable[..., subprocess.CompletedProcess[str]] | None = None,
) -> bool:
    if not _WORKSTREAM_ID.fullmatch(workstream_id):
        raise HarnessBlocked("ssh-setup-failed")
    arguments = (
        "ssh",
        material.destination,
        str(remote_executable),
        "status",
        workstream_id,
    )
    result = (
        _run_command(arguments, env=env, timeout=15)
        if runner is None
        else runner(arguments, env=env, timeout=15)
    )
    if result.returncode != 0 or len(result.stdout.encode()) > MAX_COMMAND_OUTPUT:
        return False
    return (
        "private runtime: live" in result.stdout
        and "result attention: unseen" in result.stdout
    )


def _operator_gate(message: str) -> None:
    if not sys.stdin.isatty() or not sys.stdout.isatty():
        raise HarnessBlocked("operator-terminal-unavailable")
    print(message, flush=True)
    try:
        answer = (
            input("Type y after completing this native-terminal step: ").strip().lower()
        )
    except (EOFError, KeyboardInterrupt) as error:
        raise HarnessBlocked("operator-terminal-unavailable") from error
    if answer != "y":
        raise HarnessBlocked("operator-declined")


def _set_pty_size(fd: int) -> None:
    size = struct.pack("HHHH", 50, 160, 0, 0)
    with contextlib.suppress(OSError):
        fcntl.ioctl(fd, termios.TIOCSWINSZ, size)


def _drain_pty(fd: int, stop: threading.Event) -> None:
    while not stop.is_set():
        try:
            ready, _, _ = select.select([fd], [], [], 0.25)
        except (OSError, ValueError):
            return
        if not ready:
            continue
        try:
            os.read(fd, 64 * 1024)
        except (OSError, EOFError):
            return


def _start_discarded_navigator(
    candidate: Path,
    state_root: Path,
    env: Mapping[str, str],
    cwd: Path,
) -> NavigatorProcess:
    master, slave = pty.openpty()
    _set_pty_size(slave)
    try:
        process = subprocess.Popen(
            [str(candidate), "--state-root", str(state_root), "navigator"],
            cwd=cwd,
            env=dict(env),
            stdin=slave,
            stdout=slave,
            stderr=slave,
            close_fds=True,
            start_new_session=True,
        )
    except OSError as error:
        os.close(master)
        os.close(slave)
        raise HarnessBlocked("management-command-failed") from error
    finally:
        with contextlib.suppress(OSError):
            os.close(slave)
    birth = _process_birth(process.pid)
    if birth is None:
        with contextlib.suppress(ProcessLookupError):
            os.killpg(process.pid, signal.SIGTERM)
        with contextlib.suppress(subprocess.TimeoutExpired, ProcessLookupError):
            process.wait(timeout=2)
        with contextlib.suppress(OSError):
            os.close(master)
        raise HarnessBlocked("process-identity-ambiguous")
    stop = threading.Event()
    thread = threading.Thread(target=_drain_pty, args=(master, stop), daemon=True)
    thread.start()
    return NavigatorProcess(process, master, thread, stop, birth)


def _stop_pty_drain(navigator: NavigatorProcess) -> None:
    """Stop the discard reader before closing its descriptor, then join it."""

    navigator.stop_event.set()
    with contextlib.suppress(OSError):
        os.close(navigator.master_fd)
    navigator.drain_thread.join(timeout=2)
    if navigator.drain_thread.is_alive():
        raise HarnessBlocked("cleanup-incomplete")


def _stop_navigator(navigator: NavigatorProcess) -> None:
    failure: HarnessBlocked | None = None
    try:
        current = _process_birth(navigator.process.pid)
        if current not in {None, navigator.process_birth}:
            raise HarnessBlocked("process-identity-ambiguous")
        if current is not None:
            try:
                os.killpg(navigator.process.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
            try:
                navigator.process.wait(timeout=5)
            except subprocess.TimeoutExpired as error:
                if _process_birth(navigator.process.pid) != navigator.process_birth:
                    raise HarnessBlocked("process-identity-ambiguous") from error
                os.killpg(navigator.process.pid, signal.SIGKILL)
                try:
                    navigator.process.wait(timeout=5)
                except subprocess.TimeoutExpired as final_error:
                    raise HarnessBlocked("cleanup-process-survived") from final_error
    except HarnessBlocked as error:
        failure = error
    except OSError:
        failure = HarnessBlocked("cleanup-incomplete")
    try:
        _stop_pty_drain(navigator)
    except HarnessBlocked as error:
        failure = failure or error
    if failure is not None:
        raise failure


def _run_foreground_attach(
    handle: PresentationHandle,
    runner: Callable[[Sequence[str], Mapping[str, str], float], int] | None = None,
) -> int:
    """Attach with inherited terminal streams; only management metadata is captured."""

    environment = os.environ.copy()
    environment.pop("TMUX", None)
    arguments = (
        "tmux",
        "-S",
        str(handle.socket),
        "attach-session",
        "-t",
        handle.session,
    )
    if runner is not None:
        return runner(arguments, environment, OPERATOR_ATTACH_SECONDS)

    process: subprocess.Popen[bytes] | None = None

    def terminate_process() -> None:
        if process is None or process.poll() is not None:
            return
        process.terminate()
        try:
            process.wait(timeout=2)
            return
        except subprocess.TimeoutExpired:
            process.kill()
            try:
                process.wait(timeout=2)
            except subprocess.TimeoutExpired as error:
                raise HarnessBlocked("cleanup-process-survived") from error

    try:
        # Inherited terminal streams are deliberate: provider and shell bytes
        # are never captured. The process is polled only through presentation
        # client-count metadata until the exact foreground client is attached.
        process = subprocess.Popen(
            arguments,
            env=environment,
        )
    except OSError as error:
        raise HarnessBlocked("presentation-unavailable") from error
    deadline = time.monotonic() + OPERATOR_ATTACH_SECONDS
    attached = False
    try:
        while time.monotonic() < deadline:
            returncode = process.poll()
            if returncode is not None:
                if not attached:
                    if returncode == 0:
                        raise HarnessBlocked("presentation-client-not-ready")
                    raise HarnessBlocked("presentation-unavailable")
                return returncode
            count = _presentation_client_count(handle)
            if count > 2:
                raise HarnessBlocked("presentation-client-ambiguous")
            if count == 2:
                attached = True
                break
            time.sleep(POLL_SECONDS)
        if not attached:
            raise HarnessBlocked("timeout")
        remaining = max(0.1, deadline - time.monotonic())
        try:
            return process.wait(timeout=remaining)
        except subprocess.TimeoutExpired as error:
            raise HarnessBlocked("timeout") from error
    except HarnessBlocked:
        terminate_process()
        raise


def _foreground_presentation(handle: PresentationHandle) -> None:
    if not sys.stdin.isatty() or not sys.stdout.isatty():
        raise HarnessBlocked("operator-terminal-unavailable")
    if _run_foreground_attach(handle) != 0:
        raise HarnessBlocked("presentation-unavailable")


def _path_under(root: Path, candidate: str) -> bool:
    if not candidate.startswith("/"):
        return False
    root_text = str(root.resolve())
    candidate_text = os.path.abspath(candidate)
    return candidate_text == root_text or candidate_text.startswith(root_text + os.sep)


def _root_reference_category(root: Path, owned_pids: set[int]) -> str | None:
    root_bytes = str(root.resolve()).encode()
    try:
        processes = tuple(
            entry for entry in Path("/proc").iterdir() if entry.name.isdecimal()
        )
    except OSError:
        return "ownership-ambiguous"
    for process in processes:
        pid = int(process.name)
        if pid == os.getpid() or pid in owned_pids:
            continue
        for name in ("cwd", "root", "exe"):
            try:
                if _path_under(root, os.readlink(process / name)):
                    return "ownership-ambiguous"
            except (FileNotFoundError, PermissionError, OSError):
                continue
        try:
            command_line = (process / "cmdline").read_bytes()
        except (FileNotFoundError, PermissionError, OSError):
            command_line = b""
        if root_bytes in command_line:
            return "ownership-ambiguous"
    return None


def _remove_presentation_artifact(handle: PresentationHandle) -> None:
    identifier = handle.directory.name.removeprefix("presentation-")
    if (
        handle.directory.parent.name != "presentation"
        or len(identifier) != 12
        or not re.fullmatch(r"[0-9a-f]+", identifier)
        or handle.session != f"wsnav-presentation-{identifier}"
        or handle.socket != handle.directory / "tmux.sock"
        or handle.directory.is_symlink()
    ):
        raise HarnessBlocked("ownership-ambiguous")
    if handle.socket.is_symlink():
        raise HarnessBlocked("ownership-ambiguous")
    if handle.socket.exists() and not stat.S_ISSOCK(handle.socket.stat().st_mode):
        raise HarnessBlocked("ownership-ambiguous")
    if handle.socket.exists():
        handle.socket.unlink()
    with contextlib.suppress(FileNotFoundError):
        shutil.rmtree(handle.directory)
    if handle.socket.exists() or handle.directory.exists():
        raise HarnessBlocked("cleanup-incomplete")


def _kill_private_presentation(
    handle: PresentationHandle | None,
    *,
    allow_live: bool = False,
    runner: Callable[[Sequence[str]], subprocess.CompletedProcess[str]] | None = None,
) -> None:
    if handle is None:
        return
    state = _presentation_server_state(handle, runner)
    if state == "live-other":
        raise HarnessBlocked("ownership-ambiguous")
    if state == "unknown":
        raise HarnessBlocked("ownership-ambiguous")
    if state == "absent":
        _remove_presentation_artifact(handle)
        return
    if not allow_live:
        raise HarnessBlocked("ownership-ambiguous")
    result = _tmux_probe(handle.socket, ["kill-server"], runner)
    if result.returncode != 0 and _presentation_server_state(handle, runner) == "live":
        raise HarnessBlocked("cleanup-incomplete")
    deadline = time.monotonic() + PRESENTATION_WAIT_SECONDS
    while _presentation_server_state(handle, runner) == "live":
        if time.monotonic() >= deadline:
            raise HarnessBlocked("cleanup-incomplete")
        time.sleep(POLL_SECONDS)
    _remove_presentation_artifact(handle)


def _wait_for_runtime_disappearance(
    identity: RuntimeIdentity,
    *,
    timeout: float = PRESENTATION_WAIT_SECONDS,
    birth_reader: Callable[[int], str | None] = _process_birth,
    socket_exists: Callable[[Path], bool] = Path.exists,
    sleep: Callable[[float], None] = time.sleep,
) -> str | None:
    """Wait for both the captured process identity and its private socket."""

    deadline = time.monotonic() + timeout
    same_process = False
    socket_live = False
    while time.monotonic() < deadline:
        same_process = birth_reader(identity.pane_pid) == identity.process_birth
        socket_live = socket_exists(identity.socket)
        if not same_process and not socket_live:
            return None
        sleep(min(POLL_SECONDS, max(0.0, deadline - time.monotonic())))
    if same_process:
        return "cleanup-process-survived"
    if socket_live:
        return "cleanup-incomplete"
    return None


def _cleanup_local(
    candidate: Path,
    root: DisposableRoot | None,
    fixture: LocalFixture | None,
    navigator: NavigatorProcess | None,
    presentation: PresentationHandle | None,
    runtime_identities: Sequence[RuntimeIdentity],
    ordinary_before: str,
    environment: Mapping[str, str] | None = None,
    tmux_runner: Callable[[Sequence[str]], subprocess.CompletedProcess[str]]
    | None = None,
    ordinary_fingerprint: Callable[[], str] | None = None,
) -> tuple[str, str, bool]:
    """Park exact created Workstreams, remove only owned private artifacts."""

    cleanup_error: str | None = None
    state_root = (
        fixture.state_root
        if fixture is not None
        else (root.path / "state" if root is not None else None)
    )
    cleanup_environment = (
        fixture.provider_env
        if fixture is not None
        else dict(os.environ if environment is None else environment)
    )
    fingerprint = (
        ordinary_tmux_fingerprint
        if ordinary_fingerprint is None
        else ordinary_fingerprint
    )
    discovered_presentation: PresentationHandle | None = None
    root_owned = root is None or _root_marker_valid(root)
    if not root_owned:
        cleanup_error = "ownership-ambiguous"
    if root_owned and state_root is not None:
        try:
            discovered_presentation = _presentation_artifact(state_root)
        except HarnessBlocked as error:
            cleanup_error = error.reason
    if presentation is not None:
        if discovered_presentation is None or discovered_presentation != presentation:
            cleanup_error = cleanup_error or "ownership-ambiguous"
        else:
            discovered_presentation = presentation
    navigator_stopped = False
    if navigator is not None:
        try:
            _stop_navigator(navigator)
            navigator_stopped = True
        except HarnessBlocked as error:
            cleanup_error = cleanup_error or error.reason
    if discovered_presentation is not None:
        try:
            _kill_private_presentation(
                discovered_presentation,
                allow_live=navigator_stopped,
                runner=tmux_runner,
            )
        except HarnessBlocked as error:
            cleanup_error = cleanup_error or error.reason
    if state_root is not None and state_root.exists():
        try:
            workstreams = _read_workstream_ids(state_root)
        except HarnessBlocked as error:
            workstreams = ()
            cleanup_error = cleanup_error or error.reason
        for workstream_id in workstreams:
            try:
                _read_workstream_revision(state_root, workstream_id)
                _wsnav_command(
                    candidate,
                    state_root,
                    ["park", workstream_id],
                    env=cleanup_environment,
                )
            except HarnessBlocked as error:
                cleanup_error = cleanup_error or error.reason
    for identity in runtime_identities:
        disappearance = _wait_for_runtime_disappearance(identity)
        if disappearance is not None:
            cleanup_error = cleanup_error or disappearance
    if state_root is not None:
        owned_pids = {identity.pane_pid for identity in runtime_identities}
        if navigator is not None:
            owned_pids.add(navigator.process.pid)
        reference = _root_reference_category(
            root.path if root is not None else state_root, owned_pids
        )
        if reference is not None:
            cleanup_error = cleanup_error or reference
    ordinary_after = fingerprint()
    ordinary_unchanged = (
        ordinary_before != "unavailable"
        and ordinary_after != "unavailable"
        and ordinary_after == ordinary_before
    )
    if not ordinary_unchanged:
        cleanup_error = cleanup_error or "ordinary-tmux-changed"
    root_result = "complete"
    if cleanup_error is None and root is not None:
        root_result = cleanup_disposable_root(root)
        if root_result != "complete":
            cleanup_error = root_result
    if cleanup_error is not None:
        return "incomplete", cleanup_error, ordinary_unchanged
    return "complete", "complete", ordinary_unchanged


def _wait_for_loopback_port_absence(port: int, *, timeout: float = 5) -> str | None:
    """Treat an occupied or ambiguous disposable SSH port as a cleanup failure."""

    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            with socket.create_connection(("127.0.0.1", port), timeout=0.25):
                return "cleanup-incomplete"
        except ConnectionRefusedError:
            return None
        except OSError:
            time.sleep(POLL_SECONDS)
    return "cleanup-incomplete"


def _cleanup_remote(
    candidate: Path,
    root: DisposableRoot | None,
    fixture: RemoteFixture | None,
    navigator: NavigatorProcess | None,
    presentation: PresentationHandle | None,
    runtime_identities: Sequence[RuntimeIdentity],
    ordinary_before: str,
) -> tuple[str, str, bool]:
    """Park exact remote Workstreams, stop exact SSH ownership, then remove root."""

    cleanup_error: str | None = None
    client_state_root = (
        fixture.client_state_root
        if fixture is not None
        else root.path / "client-state"
        if root is not None
        else None
    )
    cleanup_environment = (
        fixture.client_env if fixture is not None else dict(os.environ)
    )
    root_owned = root is None or _root_marker_valid(root)
    if not root_owned:
        cleanup_error = "ownership-ambiguous"
    discovered_presentation: PresentationHandle | None = None
    if root_owned and client_state_root is not None:
        try:
            discovered_presentation = _presentation_artifact(client_state_root)
        except HarnessBlocked as error:
            cleanup_error = error.reason
    if presentation is not None:
        if discovered_presentation is None or discovered_presentation != presentation:
            cleanup_error = cleanup_error or "ownership-ambiguous"
        else:
            discovered_presentation = presentation
    navigator_stopped = False
    if navigator is not None:
        try:
            _stop_navigator(navigator)
            navigator_stopped = True
        except HarnessBlocked as error:
            cleanup_error = cleanup_error or error.reason
    if discovered_presentation is not None:
        try:
            _kill_private_presentation(
                discovered_presentation,
                allow_live=navigator_stopped,
            )
        except HarnessBlocked as error:
            cleanup_error = cleanup_error or error.reason

    if fixture is not None:
        for workstream_id in fixture.workstream_ids:
            try:
                revision = _read_workstream_revision(
                    fixture.remote_state_root, workstream_id
                )
                _remote_management_command(
                    candidate,
                    fixture.client_state_root,
                    _remote_park_arguments(fixture.host_alias, workstream_id, revision),
                    env=cleanup_environment,
                    failure_reason="remote-park-failed",
                )
            except HarnessBlocked as error:
                cleanup_error = cleanup_error or error.reason

    for identity in runtime_identities:
        disappearance = _wait_for_runtime_disappearance(identity)
        if disappearance is not None:
            cleanup_error = cleanup_error or disappearance

    if fixture is not None:
        try:
            _stop_disposable_sshd(fixture.sshd)
        except HarnessBlocked as error:
            cleanup_error = cleanup_error or error.reason
        if _process_birth(fixture.sshd.process.pid) is not None:
            cleanup_error = cleanup_error or "cleanup-process-survived"
        port_error = _wait_for_loopback_port_absence(fixture.ssh_material.port)
        if port_error is not None:
            cleanup_error = cleanup_error or port_error

    if root is not None:
        owned_pids = {identity.pane_pid for identity in runtime_identities}
        if navigator is not None:
            owned_pids.add(navigator.process.pid)
        if fixture is not None:
            owned_pids.add(fixture.sshd.process.pid)
        reference = _root_reference_category(root.path, owned_pids)
        if reference is not None:
            cleanup_error = cleanup_error or reference
    ordinary_after = ordinary_tmux_fingerprint()
    ordinary_unchanged = (
        ordinary_before != "unavailable"
        and ordinary_after != "unavailable"
        and ordinary_after == ordinary_before
    )
    if not ordinary_unchanged:
        cleanup_error = cleanup_error or "ordinary-tmux-changed"
    if cleanup_error is None and root is not None:
        root_result = cleanup_disposable_root(root)
        if root_result != "complete":
            cleanup_error = root_result
    if cleanup_error is not None:
        return "incomplete", cleanup_error, ordinary_unchanged
    return "complete", "complete", ordinary_unchanged


def _machine_assertions_complete(
    assertions: Mapping[str, bool], required: frozenset[str]
) -> bool:
    return all(assertions.get(name) is True for name in required)


def _run_local_acceptance(candidate: Path) -> dict[str, Any]:
    assertions = default_assertions()
    tool_versions = default_tool_versions()
    ordinary_before = ordinary_tmux_fingerprint()
    workflow = LocalWorkflowState("setup")
    root: DisposableRoot | None = None
    fixture: LocalFixture | None = None
    navigator: NavigatorProcess | None = None
    presentation: PresentationHandle | None = None
    runtime_identities: list[RuntimeIdentity] = []
    primary_reason = "not-implemented"
    primary_status = "blocked"
    cleanup_status = "not-run"
    cleanup_reason = "not-attempted"
    ordinary_unchanged = False
    final_status = primary_status
    final_reason = primary_reason
    try:
        if not sys.stdin.isatty() or not sys.stdout.isatty():
            raise HarnessBlocked("operator-terminal-unavailable")
        if ordinary_before == "unavailable":
            raise HarnessBlocked("management-command-failed")
        _probe_candidate_abi(candidate)
        assertions["abi2_preflight"] = True
        tool_versions["wsnav"] = "checked"
        if shutil.which("tmux") is None:
            raise HarnessBlocked("management-command-failed")
        tool_versions["tmux"] = "checked"
        root = create_disposable_root()
        fixture = _build_local_fixture(candidate, root)
        runtime_a = _wait_until(
            lambda: _runtime_identity(fixture.state_root, fixture.workstream_ids[0]),
            timeout=PRESENTATION_WAIT_SECONDS,
            reason="runtime-not-live",
        )
        runtime_b = _wait_until(
            lambda: _runtime_identity(fixture.state_root, fixture.workstream_ids[1]),
            timeout=PRESENTATION_WAIT_SECONDS,
            reason="runtime-not-live",
        )
        runtime_identities.extend((runtime_a, runtime_b))
        runtime_by_index = (runtime_a, runtime_b)

        navigator = _start_discarded_navigator(
            candidate,
            fixture.state_root,
            fixture.provider_env,
            fixture.project_roots[0],
        )
        presentation, _ = _wait_for_initial_presentation(fixture.state_root)
        workflow = _advance_local_phase(workflow, "first-attach")
        print(
            "In the foreground presentation, attach either disposable Workstream "
            "natively as your first selection; "
            "make one harmless OpenCode turn and wait for its normal completed "
            'state; create one shell with Ctrl+b "; run '
            './.wsnav-d12-check hostname, pwd, and git; repeat Ctrl+b "; '
            "try Ctrl+b % and guarded x from Navigator/provider; leave the "
            "utility open, then detach with Ctrl+b d.",
            flush=True,
        )
        _foreground_presentation(presentation)
        _operator_gate("Confirm the native turn, shell checks, and detach completed.")
        presentation = _wait_until(
            lambda: _presentation_handle(fixture.state_root),
            timeout=PRESENTATION_WAIT_SECONDS,
            reason="presentation-unavailable",
        )
        selected_index = _wait_for_running_attachment(
            presentation,
            fixture.workstream_ids,
        )
        selected_workstream_id = fixture.workstream_ids[selected_index]
        selected_project_root = fixture.project_roots[selected_index]
        selected_sentinels = fixture.sentinel_paths[selected_index]
        if (
            _runtime_identity(fixture.state_root, selected_workstream_id)
            != runtime_by_index[selected_index]
        ):
            raise HarnessBlocked("runtime-not-live")
        _wait_until(
            lambda: _status_has_result_attention(
                candidate,
                fixture.state_root,
                selected_workstream_id,
                fixture.provider_env,
            ),
            timeout=PRESENTATION_WAIT_SECONDS,
            reason="timeout",
        )
        topology = _presentation_topology(presentation)
        if len(topology) != 3:
            raise HarnessBlocked("topology-invalid")
        provider = next(
            (record for record in topology if record.role == "provider"), None
        )
        if provider is None or (
            provider.host_alias != "local"
            or provider.workstream_id != selected_workstream_id
        ):
            raise HarnessBlocked("topology-context-invalid")
        utility_before = _utility_metadata(presentation)
        if (
            utility_before.host_alias != "local"
            or utility_before.workstream_id != selected_workstream_id
        ):
            raise HarnessBlocked("utility-context-invalid")
        if utility_before.current_path != selected_project_root.resolve():
            raise HarnessBlocked("utility-cwd-invalid")
        if not all(path.exists() for path in selected_sentinels.values()):
            raise HarnessBlocked("sentinel-missing")
        assertions.update(
            {
                "local_below_provider_geometry": True,
                "local_canonical_cwd": True,
                "local_git_status": True,
                "local_one_shell_idempotent": True,
                "local_provider_interactive": True,
                "local_running_attachment": True,
                "local_topology": True,
            }
        )

        workflow = _advance_local_phase(workflow, "provider-switch")
        print(
            "Reattach the same presentation; while the utility remains open, "
            "switch natively to the other disposable Workstream, verify the shell "
            "is unchanged, "
            "then detach. Do not type provider input through any management path.",
            flush=True,
        )
        _foreground_presentation(presentation)
        _operator_gate("Confirm native provider switching and detach completed.")
        presentation = _wait_until(
            lambda: _presentation_handle(fixture.state_root),
            timeout=PRESENTATION_WAIT_SECONDS,
            reason="presentation-unavailable",
        )
        _wait_for_running_attachment(
            presentation,
            fixture.workstream_ids,
            expected_index=1 - selected_index,
        )
        expected_workstream_id = fixture.workstream_ids[1 - selected_index]
        switched_topology = _presentation_topology(presentation)
        switched_provider = next(
            (record for record in switched_topology if record.role == "provider"),
            None,
        )
        if switched_provider is None or (
            switched_provider.host_alias != "local"
            or switched_provider.workstream_id != expected_workstream_id
        ):
            raise HarnessBlocked("topology-context-invalid")
        utility_after = _utility_metadata(presentation)
        utility_difference = _utility_metadata_difference(utility_before, utility_after)
        if utility_difference is not None:
            raise HarnessBlocked(utility_difference)
        for index, workstream_id in enumerate(fixture.workstream_ids):
            if (
                _runtime_identity(fixture.state_root, workstream_id)
                != runtime_by_index[index]
            ):
                raise HarnessBlocked("runtime-not-live")
        assertions.update(
            {
                "local_detach_reattach": True,
                "local_runtime_identity": True,
                "local_utility_context_fixed": True,
            }
        )

        workflow = _advance_local_phase(workflow, "shell-exit")
        print(
            "Reattach, focus the utility, and press Ctrl+d. Detach after it is "
            "gone and the exact two-pane layout is visible.",
            flush=True,
        )
        _foreground_presentation(presentation)
        _operator_gate("Confirm Ctrl+d restored the two-pane layout.")
        presentation = _wait_until(
            lambda: _presentation_handle(fixture.state_root),
            timeout=PRESENTATION_WAIT_SECONDS,
            reason="presentation-unavailable",
        )
        if len(_presentation_topology(presentation)) != 2:
            raise HarnessBlocked("topology-invalid")
        assertions["local_shell_exit_cleanup"] = True

        workflow = _advance_local_phase(workflow, "guarded-close")
        print(
            "Reattach, create one utility again, verify Ctrl+b x from Navigator "
            "and provider does not change layout, then focus utility and confirm "
            "x closes only that utility; detach afterward.",
            flush=True,
        )
        _foreground_presentation(presentation)
        _operator_gate("Confirm guarded close behavior and final two-pane layout.")
        presentation = _wait_until(
            lambda: _presentation_handle(fixture.state_root),
            timeout=PRESENTATION_WAIT_SECONDS,
            reason="presentation-unavailable",
        )
        if len(_presentation_topology(presentation)) != 2:
            raise HarnessBlocked("topology-invalid")
        assertions["local_guarded_close"] = True
        assertions["privacy_bounded"] = True
    except HarnessBlocked as error:
        primary_reason = error.reason
        primary_status = "blocked"
    except (OSError, sqlite3.Error, ValueError, KeyboardInterrupt) as error:
        primary_reason = bounded_category(error)
        primary_status = "blocked"
    finally:
        workflow = _begin_local_cleanup(workflow)
        try:
            cleanup_status, cleanup_reason, ordinary_unchanged = _cleanup_local(
                candidate,
                root,
                fixture,
                navigator,
                presentation,
                runtime_identities,
                ordinary_before,
            )
        except HarnessBlocked as error:
            cleanup_status, cleanup_reason = "incomplete", error.reason
        except (OSError, sqlite3.Error, ValueError) as error:
            cleanup_status, cleanup_reason = "incomplete", bounded_category(error)
        assertions["ordinary_tmux_unchanged"] = ordinary_unchanged
        assertions["cleanup_complete"] = cleanup_status == "complete"
        if cleanup_status != "complete":
            final_status = "falsified"
            final_reason = cleanup_reason
        else:
            if primary_reason == "not-implemented" and _machine_assertions_complete(
                assertions, LOCAL_MACHINE_ASSERTIONS
            ):
                primary_reason = "visual-confirmation-required"
            final_status = primary_status
            final_reason = primary_reason
    return make_result(
        status=final_status,
        reason=final_reason,
        operator_confirmed=True,
        primary_status=primary_status,
        primary_reason=primary_reason,
        assertions=assertions,
        tool_versions=tool_versions,
        cleanup_status=cleanup_status,
        cleanup_reason=cleanup_reason,
    )


def _run_remote_acceptance(candidate: Path) -> dict[str, Any]:
    assertions = default_assertions()
    tool_versions = default_tool_versions()
    ordinary_before = ordinary_tmux_fingerprint()
    root: DisposableRoot | None = None
    fixture: RemoteFixture | None = None
    navigator: NavigatorProcess | None = None
    presentation: PresentationHandle | None = None
    runtime_identities: list[RuntimeIdentity] = []
    primary_reason = "not-implemented"
    primary_status = "blocked"
    cleanup_status = "not-run"
    cleanup_reason = "not-attempted"
    ordinary_unchanged = False
    final_status = primary_status
    final_reason = primary_reason
    try:
        if not sys.stdin.isatty() or not sys.stdout.isatty():
            raise HarnessBlocked("operator-terminal-unavailable")
        if ordinary_before == "unavailable":
            raise HarnessBlocked("management-command-failed")
        _probe_candidate_abi(candidate)
        assertions["abi2_preflight"] = True
        tool_versions["wsnav"] = "checked"
        if shutil.which("tmux") is None:
            raise HarnessBlocked("management-command-failed")
        tool_versions["tmux"] = "checked"
        root = create_disposable_root(prefix="wsnav-d12-remote.")
        fixture = _build_remote_fixture(candidate, root)
        tool_versions["ssh"] = "checked"
        runtime_a = _wait_until(
            lambda: _runtime_identity(
                fixture.remote_state_root, fixture.workstream_ids[0]
            ),
            timeout=PRESENTATION_WAIT_SECONDS,
            reason="runtime-not-live",
        )
        runtime_b = _wait_until(
            lambda: _runtime_identity(
                fixture.remote_state_root, fixture.workstream_ids[1]
            ),
            timeout=PRESENTATION_WAIT_SECONDS,
            reason="runtime-not-live",
        )
        runtime_identities.extend((runtime_a, runtime_b))
        runtime_by_index = (runtime_a, runtime_b)
        navigator = _start_discarded_navigator(
            candidate,
            fixture.client_state_root,
            fixture.client_env,
            root.path,
        )
        presentation, _ = _wait_for_initial_presentation(fixture.client_state_root)
        print(
            "In the foreground remote presentation, attach either disposable "
            "remote Workstream natively; make one harmless provider turn and "
            'wait for its normal completed state; create one shell with Ctrl+b "; '
            'run ./.wsnav-d12-check hostname, pwd, and git; repeat Ctrl+b "; '
            "try Ctrl+b % and guarded x from Navigator/provider; leave the "
            "utility open, then detach with Ctrl+b d.",
            flush=True,
        )
        _foreground_presentation(presentation)
        _operator_gate(
            "Confirm the remote native turn, shell checks, and detach completed."
        )
        presentation = _wait_until(
            lambda: _presentation_handle(fixture.client_state_root),
            timeout=PRESENTATION_WAIT_SECONDS,
            reason="presentation-unavailable",
        )
        selected_index = _wait_for_running_attachment(
            presentation,
            fixture.workstream_ids,
            expected_host=fixture.host_alias,
        )
        selected_workstream_id = fixture.workstream_ids[selected_index]
        selected_sentinels = fixture.sentinel_paths[selected_index]
        if (
            _runtime_identity(fixture.remote_state_root, selected_workstream_id)
            != runtime_by_index[selected_index]
        ):
            raise HarnessBlocked("runtime-not-live")
        _wait_until(
            lambda: _remote_status_has_result_attention(
                fixture.ssh_material,
                fixture.remote_executable,
                selected_workstream_id,
                fixture.client_env,
            ),
            timeout=PRESENTATION_WAIT_SECONDS,
            reason="runtime-not-live",
        )
        topology = _presentation_topology(presentation)
        if len(topology) != 3:
            raise HarnessBlocked("topology-invalid")
        provider = next(
            (record for record in topology if record.role == "provider"), None
        )
        if provider is None or (
            provider.host_alias != fixture.host_alias
            or provider.workstream_id != selected_workstream_id
        ):
            raise HarnessBlocked("topology-context-invalid")
        utility_before = _utility_metadata(presentation)
        if (
            utility_before.host_alias != fixture.host_alias
            or utility_before.workstream_id != selected_workstream_id
        ):
            raise HarnessBlocked("utility-context-invalid")
        if not all(path.exists() for path in selected_sentinels.values()):
            raise HarnessBlocked("sentinel-missing")
        assertions.update(
            {
                "ssh_below_provider_geometry": True,
                "ssh_canonical_cwd": True,
                "ssh_git_status": True,
                "ssh_one_shell_idempotent": True,
                "ssh_provider_interactive": True,
                "ssh_running_attachment": True,
                "ssh_topology": True,
            }
        )

        print(
            "Reattach the same remote presentation; while the utility remains "
            "open, switch natively to the other remote Workstream, verify the "
            "shell is unchanged, then detach. Do not type provider input through "
            "any management path.",
            flush=True,
        )
        _foreground_presentation(presentation)
        _operator_gate("Confirm remote native switching and detach completed.")
        presentation = _wait_until(
            lambda: _presentation_handle(fixture.client_state_root),
            timeout=PRESENTATION_WAIT_SECONDS,
            reason="presentation-unavailable",
        )
        _wait_for_running_attachment(
            presentation,
            fixture.workstream_ids,
            expected_host=fixture.host_alias,
            expected_index=1 - selected_index,
        )
        expected_workstream_id = fixture.workstream_ids[1 - selected_index]
        switched_topology = _presentation_topology(presentation)
        switched_provider = next(
            (record for record in switched_topology if record.role == "provider"),
            None,
        )
        if switched_provider is None or (
            switched_provider.host_alias != fixture.host_alias
            or switched_provider.workstream_id != expected_workstream_id
        ):
            raise HarnessBlocked("topology-context-invalid")
        utility_after = _utility_metadata(presentation)
        utility_difference = _utility_metadata_difference(utility_before, utility_after)
        if utility_difference is not None:
            raise HarnessBlocked(utility_difference)
        for index, workstream_id in enumerate(fixture.workstream_ids):
            if (
                _runtime_identity(fixture.remote_state_root, workstream_id)
                != runtime_by_index[index]
            ):
                raise HarnessBlocked("runtime-not-live")
        assertions.update(
            {
                "ssh_detach_reattach": True,
                "ssh_runtime_identity": True,
                "ssh_utility_context_fixed": True,
            }
        )

        print(
            "Reattach, focus the remote utility, and press Ctrl+d. Detach after "
            "it is gone and the exact two-pane layout is visible.",
            flush=True,
        )
        _foreground_presentation(presentation)
        _operator_gate("Confirm remote Ctrl+d restored the two-pane layout.")
        presentation = _wait_until(
            lambda: _presentation_handle(fixture.client_state_root),
            timeout=PRESENTATION_WAIT_SECONDS,
            reason="presentation-unavailable",
        )
        if len(_presentation_topology(presentation)) != 2:
            raise HarnessBlocked("topology-invalid")
        assertions["ssh_shell_exit_cleanup"] = True

        print(
            "Reattach, create one remote utility again, verify Ctrl+b x from "
            "Navigator and provider does not change layout, then focus utility "
            "and confirm x closes only that utility; detach afterward.",
            flush=True,
        )
        _foreground_presentation(presentation)
        _operator_gate(
            "Confirm remote guarded close behavior and final two-pane layout."
        )
        presentation = _wait_until(
            lambda: _presentation_handle(fixture.client_state_root),
            timeout=PRESENTATION_WAIT_SECONDS,
            reason="presentation-unavailable",
        )
        if len(_presentation_topology(presentation)) != 2:
            raise HarnessBlocked("topology-invalid")
        assertions["ssh_guarded_close"] = True
        assertions["privacy_bounded"] = True
    except HarnessBlocked as error:
        primary_reason = error.reason
        primary_status = "blocked"
    except (OSError, sqlite3.Error, ValueError, KeyboardInterrupt) as error:
        primary_reason = bounded_category(error)
        primary_status = "blocked"
    finally:
        try:
            cleanup_status, cleanup_reason, ordinary_unchanged = _cleanup_remote(
                candidate,
                root,
                fixture,
                navigator,
                presentation,
                runtime_identities,
                ordinary_before,
            )
        except HarnessBlocked as error:
            cleanup_status, cleanup_reason = "incomplete", error.reason
        except (OSError, sqlite3.Error, ValueError) as error:
            cleanup_status, cleanup_reason = "incomplete", bounded_category(error)
        assertions["ordinary_tmux_unchanged"] = ordinary_unchanged
        assertions["cleanup_complete"] = cleanup_status == "complete"
        if cleanup_status != "complete":
            final_status = "falsified"
            final_reason = cleanup_reason
        else:
            if primary_reason == "not-implemented" and _machine_assertions_complete(
                assertions, REMOTE_MACHINE_ASSERTIONS
            ):
                primary_reason = "visual-confirmation-required"
            final_status = primary_status
            final_reason = primary_reason
    return make_result(
        status=final_status,
        reason=final_reason,
        operator_confirmed=True,
        primary_status=primary_status,
        primary_reason=primary_reason,
        assertions=assertions,
        tool_versions=tool_versions,
        cleanup_status=cleanup_status,
        cleanup_reason=cleanup_reason,
    )


def _local_machine_ready_for_remote(result: Mapping[str, Any]) -> bool:
    if result.get("status") == "falsified":
        return False
    if result.get("cleanup", {}).get("status") != "complete":
        return False
    assertions = result.get("assertions")
    if not isinstance(assertions, Mapping):
        return False
    return _machine_assertions_complete(assertions, LOCAL_MACHINE_ASSERTIONS)


def _combine_acceptance_results(
    local: Mapping[str, Any], remote: Mapping[str, Any]
) -> dict[str, Any]:
    assertions = dict(local["assertions"])
    assertions.update(
        {
            name: value
            for name, value in remote["assertions"].items()
            if name.startswith("ssh_")
        }
    )
    tool_versions = {
        name: "checked"
        if local["tool_versions"].get(name) == "checked"
        or remote["tool_versions"].get(name) == "checked"
        else "not-run"
        for name in ("ssh", "tmux", "wsnav")
    }
    local_cleanup = local["cleanup"]
    remote_cleanup = remote["cleanup"]
    cleanup_complete = (
        local_cleanup.get("status") == "complete"
        and remote_cleanup.get("status") == "complete"
    )
    assertions["cleanup_complete"] = cleanup_complete
    assertions["ordinary_tmux_unchanged"] = bool(
        assertions.get("ordinary_tmux_unchanged")
        and remote["assertions"].get("ordinary_tmux_unchanged")
    )
    machine_complete = cleanup_complete and _machine_assertions_complete(
        assertions, LOCAL_MACHINE_ASSERTIONS | REMOTE_MACHINE_ASSERTIONS
    )
    if machine_complete:
        primary_status = "blocked"
        primary_reason = "visual-confirmation-required"
    elif (
        remote.get("status") == "falsified"
        or remote.get("primary_reason") != "not-implemented"
    ):
        primary_status = remote["primary_status"]
        primary_reason = remote["primary_reason"]
    else:
        primary_status = local["primary_status"]
        primary_reason = local["primary_reason"]
    if not cleanup_complete:
        status = "falsified"
        reason = (
            remote_cleanup.get("reason")
            if remote_cleanup.get("status") != "complete"
            else local_cleanup.get("reason")
        )
    elif machine_complete:
        status = "blocked"
        reason = "visual-confirmation-required"
    elif remote.get("status") == "falsified":
        status = "falsified"
        reason = remote["reason"]
    elif remote.get("primary_reason") != "not-implemented":
        status = remote["status"]
        reason = remote["reason"]
    else:
        status = "blocked"
        reason = "not-implemented"
    return make_result(
        status=status,
        reason=reason,
        operator_confirmed=True,
        primary_status=primary_status,
        primary_reason=primary_reason,
        assertions=assertions,
        tool_versions=tool_versions,
        cleanup_status="complete" if cleanup_complete else "incomplete",
        cleanup_reason=(
            "complete"
            if cleanup_complete
            else remote_cleanup.get("reason", "cleanup-incomplete")
        ),
    )


def bounded_category(error: BaseException | str) -> str:
    if isinstance(error, HarnessBlocked) and error.reason in ALLOWED_REASONS:
        return error.reason
    if isinstance(error, str) and error in ALLOWED_REASONS:
        return error
    return "internal-error"


def live_effect_seam(_candidate: Path) -> None:
    """The explicit handoff boundary for the later operator harness slice."""

    raise HarnessBlocked("not-implemented")


def confirmed_foundation_result(workspace: Path) -> dict[str, Any]:
    try:
        candidate = resolve_candidate_binary(workspace)
        live_effect_seam(candidate)
    except HarnessBlocked as error:
        return make_result(
            status="blocked", reason=error.reason, operator_confirmed=True
        )
    except Exception as error:  # noqa: BLE001 - sanitize every foundation failure.
        return make_result(
            status="blocked", reason=bounded_category(error), operator_confirmed=True
        )
    raise AssertionError("live effect seam unexpectedly returned")


def run_self_test() -> None:
    parsed = parse_arguments([])
    assert not parsed.confirm_live and not parsed.self_test
    assert parse_arguments(["--confirm-live-d12"]).confirm_live

    blocked = make_result(
        status="blocked",
        reason="operator-confirmation-required",
        operator_confirmed=False,
    )
    assert all(value is False for value in blocked["assertions"].values())
    assert blocked["primary_status"] == "blocked"
    assert blocked["primary_reason"] == "operator-confirmation-required"
    preserved = make_result(
        status="falsified",
        reason="cleanup-incomplete",
        primary_status="blocked",
        primary_reason="provider-unavailable",
        operator_confirmed=True,
    )
    assert preserved["status"] == "falsified"
    assert preserved["reason"] == "cleanup-incomplete"
    assert preserved["primary_status"] == "blocked"
    assert preserved["primary_reason"] == "provider-unavailable"
    with tempfile.TemporaryDirectory(prefix="wsnav-d12-self-test.") as temporary:
        result_path = Path(temporary) / "result.json"
        write_result(result_path, blocked)
        assert stat.S_IMODE(result_path.stat().st_mode) == 0o600
        assert json.loads(result_path.read_text(encoding="utf-8")) == blocked

        candidate_root = Path(temporary) / "checkout"
        candidate = candidate_root / "target" / "debug" / "wsnav"
        candidate.parent.mkdir(parents=True)
        candidate.write_bytes(b"fixture")
        candidate.chmod(0o700)
        assert resolve_candidate_binary(candidate_root) == candidate.resolve()
        confirmed = confirmed_foundation_result(candidate_root)
        assert confirmed["reason"] == "not-implemented"
        assert confirmed["operator_confirmed"] is True

        status_workstream = "01234567-89ab-4abc-8def-0123456789ab"
        remote_project = Path("/disposable/remote-project")
        remote_executable = Path("/disposable/remote-wrapper")
        assert _remote_register_arguments(
            "loopback-d12", "wsnav-d12-loopback", remote_executable
        ) == (
            "register-remote",
            "loopback-d12",
            "--destination",
            "wsnav-d12-loopback",
            "--executable",
            str(remote_executable),
        )
        assert _remote_checkout_arguments("loopback-d12", remote_project) == (
            "host",
            "register-checkout",
            "loopback-d12",
            str(remote_project),
            "--provider",
            "opencode",
        )
        assert _remote_start_arguments("loopback-d12", status_workstream, 1) == (
            "host",
            "start",
            "loopback-d12",
            status_workstream,
            "1",
        )
        assert _remote_park_arguments("loopback-d12", status_workstream, 2) == (
            "host",
            "park",
            "loopback-d12",
            status_workstream,
            "2",
        )

        def stage_failure(
            _candidate: Path,
            _state_root: Path,
            _arguments: Sequence[str],
            _environment: Mapping[str, str],
        ) -> str:
            raise HarnessBlocked("management-command-failed")

        try:
            _remote_management_command(
                candidate,
                Path(temporary) / "client-state",
                _remote_start_arguments("loopback-d12", status_workstream, 1),
                env={},
                failure_reason="remote-start-failed",
                runner=stage_failure,
            )
        except HarnessBlocked as error:
            assert error.reason == "remote-start-failed"
        else:
            raise AssertionError("remote setup failure lost its stage category")
        for failure_reason, arguments in (
            (
                "remote-host-registration-failed",
                _remote_register_arguments(
                    "loopback-d12", "wsnav-d12-loopback", remote_executable
                ),
            ),
            (
                "remote-checkout-registration-failed",
                _remote_checkout_arguments("loopback-d12", remote_project),
            ),
            (
                "remote-park-failed",
                _remote_park_arguments("loopback-d12", status_workstream, 2),
            ),
        ):
            try:
                _remote_management_command(
                    candidate,
                    Path(temporary) / "client-state",
                    arguments,
                    env={},
                    failure_reason=failure_reason,
                    runner=stage_failure,
                )
            except HarnessBlocked as error:
                assert error.reason == failure_reason
            else:
                raise AssertionError("remote setup stage lost its category")

        def stage_success(
            _candidate: Path,
            _state_root: Path,
            _arguments: Sequence[str],
            _environment: Mapping[str, str],
        ) -> str:
            return "registered workstream " + status_workstream

        assert (
            _remote_management_command(
                candidate,
                Path(temporary) / "client-state",
                _remote_checkout_arguments("loopback-d12", remote_project),
                env={},
                failure_reason="remote-checkout-registration-failed",
                runner=stage_success,
            )
            == "registered workstream " + status_workstream
        )

        ssh_root = Path(temporary) / "ssh'root#{client}#(marker)"
        port = _free_loopback_port()
        material = _ssh_material(
            ssh_root,
            port=port,
            username="wsnav.test",
            destination="wsnav-d12-loopback",
        )
        assert 1024 < port < 65536
        sshd_config = _sshd_config_text(material)
        assert "ListenAddress 127.0.0.1\n" in sshd_config
        assert f"HostKey {material.host_key}\n" in sshd_config
        assert f"AuthorizedKeysFile {material.authorized_keys}\n" in sshd_config
        assert "PasswordAuthentication no\n" in sshd_config
        assert "AllowTcpForwarding no\n" in sshd_config
        client_config = _ssh_client_config_text(material)
        assert "StrictHostKeyChecking yes\n" in client_config
        assert "GlobalKnownHostsFile none\n" in client_config
        assert "IdentityAgent none\n" in client_config
        assert f"HostKeyAlias {material.destination}\n" in client_config
        assert f"UserKnownHostsFile {material.known_hosts}\n" in client_config
        assert "/dev/null" not in client_config
        client_wrapper = _ssh_client_wrapper_text(material, Path("/usr/bin/ssh"))
        assert f' -F {shlex.quote(str(material.client_config))} "$@"' in client_wrapper
        assert client_wrapper.startswith("#!/bin/sh\nset -eu\nexec ")
        try:
            _ssh_client_wrapper_text(material, Path("ssh"))
        except HarnessBlocked as error:
            assert error.reason == "ssh-setup-failed"
        else:
            raise AssertionError("relative SSH client escaped the fixed wrapper")
        client_environment = _ssh_client_environment(material)
        assert client_environment.get("HOME") == os.environ.get("HOME")
        assert "SSH_AUTH_SOCK" not in client_environment
        assert "TMUX" not in client_environment
        assert client_environment["PATH"].split(os.pathsep, 1)[0] == str(
            material.client_wrapper.parent
        )

        scanned = (
            "# disposable scanner evidence\n"
            f"[127.0.0.1]:{material.port} ssh-ed25519 AAAA\n"
            f"[127.0.0.1]:{material.port} ssh-ed25519 AAAA\n"
        )
        assert _known_hosts_from_keyscan(material, scanned) == (
            f"{material.destination} ssh-ed25519 AAAA\n"
        )
        try:
            _known_hosts_from_keyscan(material, "[127.0.0.2]:1 ssh-ed25519 AAAA\n")
        except HarnessBlocked as error:
            assert error.reason == "ssh-daemon-unavailable"
        else:
            raise AssertionError("foreign keyscan endpoint was accepted")
        try:
            _known_hosts_from_keyscan(
                material, f"[127.0.0.1]:{material.port} ssh-ed25519 !invalid!\n"
            )
        except HarnessBlocked as error:
            assert error.reason == "ssh-daemon-unavailable"
        else:
            raise AssertionError("invalid keyscan key was accepted")

        special_remote_root = Path(temporary) / "remote's#{state}#(marker)"
        remote_environment = {
            name: str(special_remote_root / suffix)
            for name, suffix in (
                ("XDG_CONFIG_HOME", "config"),
                ("XDG_DATA_HOME", "data"),
                ("XDG_CACHE_HOME", "cache"),
                ("XDG_STATE_HOME", "state"),
            )
        }
        remote_environment["PATH"] = "/usr/bin:/bin"
        remote_candidate = special_remote_root / "bin" / "wsnav"
        remote_wrapper = _remote_wrapper_text(
            remote_candidate, special_remote_root / "host-state", remote_environment
        )
        for name in (
            "XDG_CONFIG_HOME",
            "XDG_DATA_HOME",
            "XDG_CACHE_HOME",
            "XDG_STATE_HOME",
        ):
            assert f"export {name}=" in remote_wrapper
        assert f"exec {shlex.quote(str(remote_candidate.resolve()))}" in remote_wrapper
        assert '"$@"' in remote_wrapper
        assert "auth.json" not in remote_wrapper
        remote_wrapper_path = special_remote_root / "bin" / "remote-wrapper"
        _write_executable(remote_wrapper_path, remote_wrapper)
        assert stat.S_IMODE(remote_wrapper_path.stat().st_mode) == 0o700
        syntax = _run_command(["sh", "-n", remote_wrapper_path], timeout=5)
        assert syntax.returncode == 0
        try:
            _remote_wrapper_text(
                Path("relative-wsnav"),
                special_remote_root / "host-state",
                remote_environment,
            )
        except HarnessBlocked as error:
            assert error.reason == "ssh-setup-failed"
        else:
            raise AssertionError("relative remote wrapper escaped the fixed seam")
        relative_environment = dict(remote_environment)
        relative_environment["XDG_STATE_HOME"] = "ordinary-state"
        try:
            _remote_wrapper_text(
                remote_candidate,
                special_remote_root / "host-state",
                relative_environment,
            )
        except HarnessBlocked as error:
            assert error.reason == "ssh-setup-failed"
        else:
            raise AssertionError("ordinary relative remote state was accepted")

        probe_calls: list[tuple[tuple[str, ...], Mapping[str, str], float]] = []

        def probe_runner(
            arguments: Sequence[str],
            *,
            env: Mapping[str, str],
            timeout: float,
        ) -> subprocess.CompletedProcess[str]:
            probe_calls.append((tuple(arguments), env, timeout))
            return subprocess.CompletedProcess(
                arguments,
                0,
                '{"control_abi":2,"protocol_version":18,"host_schema_version":12}\n',
                "",
            )

        remote_executable = Path("/opt/wsnav-d12/remote-wrapper")
        _probe_remote_abi(
            candidate.resolve(),
            material,
            remote_executable,
            client_environment,
            runner=probe_runner,
        )
        assert probe_calls == [
            (
                ("ssh", material.destination, str(remote_executable), "_probe"),
                client_environment,
                15,
            )
        ]
        assert str(candidate_root) not in probe_calls[0][0]
        status_calls: list[tuple[str, ...]] = []

        def status_runner(
            arguments: Sequence[str],
            *,
            env: Mapping[str, str],
            timeout: float,
        ) -> subprocess.CompletedProcess[str]:
            status_calls.append(tuple(arguments))
            return subprocess.CompletedProcess(
                arguments,
                0,
                "private runtime: live\nresult attention: unseen\n",
                "",
            )

        assert _remote_status_has_result_attention(
            material,
            remote_executable,
            status_workstream,
            client_environment,
            runner=status_runner,
        )
        assert status_calls == [
            (
                "ssh",
                material.destination,
                str(remote_executable),
                "status",
                status_workstream,
            )
        ]
        try:
            _remote_status_has_result_attention(
                material,
                remote_executable,
                "not-a-workstream",
                client_environment,
                runner=status_runner,
            )
        except HarnessBlocked as error:
            assert error.reason == "ssh-setup-failed"
        else:
            raise AssertionError("unvalidated remote status ID was accepted")

        def wrong_abi_runner(
            arguments: Sequence[str],
            *,
            env: Mapping[str, str],
            timeout: float,
        ) -> subprocess.CompletedProcess[str]:
            return subprocess.CompletedProcess(
                arguments,
                0,
                '{"control_abi":1,"protocol_version":18,"host_schema_version":12}\n',
                "",
            )

        try:
            _probe_remote_abi(
                candidate.resolve(),
                material,
                remote_executable,
                client_environment,
                runner=wrong_abi_runner,
            )
        except HarnessBlocked as error:
            assert error.reason == "candidate-not-current-checkout"
        else:
            raise AssertionError("ABI 1 remote probe was accepted")
        try:
            _probe_remote_abi(
                Path("relative-wsnav"),
                material,
                remote_executable,
                client_environment,
                runner=probe_runner,
            )
        except HarnessBlocked as error:
            assert error.reason == "ssh-setup-failed"
        else:
            raise AssertionError("relative candidate crossed the probe seam")
        for field, value in (("username", "-oProxyCommand=bad"), ("destination", "--")):
            kwargs = {
                "port": port,
                "username": "wsnav.test",
                "destination": "wsnav-d12-loopback",
            }
            kwargs[field] = value
            try:
                _ssh_material(ssh_root, **kwargs)
            except HarnessBlocked as error:
                assert error.reason == "ssh-setup-failed"
            else:
                raise AssertionError("unsafe SSH identity value was accepted")

        owned = create_disposable_root()
        assert stat.S_IMODE(owned.path.stat().st_mode) == 0o700
        assert cleanup_disposable_root(owned) == "complete"
        assert not owned.path.exists()

        ambiguous = create_disposable_root()
        (ambiguous.path / ROOT_MARKER).unlink()
        assert cleanup_disposable_root(ambiguous) == "ownership-ambiguous"
        assert ambiguous.path.exists()
        (ambiguous.path / ROOT_MARKER).write_text(ROOT_MARKER_CONTENT, encoding="ascii")
        assert cleanup_disposable_root(ambiguous) == "complete"

        presentation_directory = Path(temporary) / "presentation-abcdef012345"
        presentation_directory.mkdir()
        handle = PresentationHandle(
            Path(temporary) / "tmux.sock",
            "wsnav-presentation-abcdef012345",
            presentation_directory,
        )
        attachment = presentation_directory / "attachment.json"
        workstream_a = "01234567-89ab-4cde-8123-456789abcdef"
        workstream_b = "fedcba98-7654-4321-8765-0123456789ab"
        attempt_id = "11111111-2222-4333-8444-555555555555"
        try:
            _select_running_attachment(handle, (workstream_a, workstream_b))
        except HarnessBlocked as error:
            assert error.reason == "attachment-not-running"
        else:
            raise AssertionError("missing attachment was accepted")
        attachment.write_text(
            json.dumps(
                {
                    "attempt_id": attempt_id,
                    "host_alias": "local",
                    "workstream_id": workstream_a,
                    "phase": "running",
                }
            ),
            encoding="ascii",
        )
        assert _attachment_status(handle, workstream_a)
        assert not _attachment_status(handle, workstream_b)
        assert _select_running_attachment(handle, (workstream_a, workstream_b)) == 0
        attachment.write_text(
            json.dumps(
                {
                    "attempt_id": attempt_id,
                    "host_alias": "local",
                    "workstream_id": workstream_b,
                    "phase": "running",
                }
            ),
            encoding="ascii",
        )
        assert _select_running_attachment(handle, (workstream_a, workstream_b)) == 1
        unknown_workstream = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee"
        attachment.write_text(
            json.dumps(
                {
                    "attempt_id": attempt_id,
                    "host_alias": "local",
                    "workstream_id": unknown_workstream,
                    "phase": "running",
                }
            ),
            encoding="ascii",
        )
        try:
            _select_running_attachment(handle, (workstream_a, workstream_b))
        except HarnessBlocked as error:
            assert error.reason == "attachment-workstream-unknown"
        else:
            raise AssertionError("unknown attachment identity was accepted")
        attachment.write_text(
            json.dumps(
                {
                    "attempt_id": attempt_id,
                    "host_alias": "local",
                    "workstream_id": workstream_b,
                    "phase": "pending",
                }
            ),
            encoding="ascii",
        )
        try:
            _select_running_attachment(handle, (workstream_a, workstream_b))
        except HarnessBlocked as error:
            assert error.reason == "attachment-not-running"
        else:
            raise AssertionError("non-running attachment was accepted")
        attachment.write_text(
            json.dumps(
                {
                    "attempt_id": attempt_id,
                    "host_alias": "remote",
                    "workstream_id": workstream_b,
                    "phase": "running",
                }
            ),
            encoding="ascii",
        )
        try:
            _select_running_attachment(handle, (workstream_a, workstream_b))
        except HarnessBlocked as error:
            assert error.reason == "attachment-host-invalid"
        else:
            raise AssertionError("wrong attachment host was accepted")
        attachment.write_text("{}", encoding="ascii")
        assert not _attachment_status(handle, workstream_a)
        try:
            _select_running_attachment(handle, (workstream_a, workstream_b))
        except HarnessBlocked as error:
            assert error.reason == "attachment-parse-invalid"
        else:
            raise AssertionError("malformed attachment was accepted")
        attachment.write_bytes(b"\xff")
        try:
            _select_running_attachment(handle, (workstream_a, workstream_b))
        except HarnessBlocked as error:
            assert error.reason == "attachment-parse-invalid"
        else:
            raise AssertionError("non-UTF-8 attachment was accepted")
        attachment.write_text(
            json.dumps(
                {
                    "attempt_id": "not-an-attempt",
                    "host_alias": "local",
                    "workstream_id": workstream_b,
                    "phase": "running",
                }
            ),
            encoding="ascii",
        )
        try:
            _select_running_attachment(handle, (workstream_a, workstream_b))
        except HarnessBlocked as error:
            assert error.reason == "attachment-attempt-invalid"
        else:
            raise AssertionError("invalid attachment attempt was accepted")
        attachment.write_text(
            json.dumps(
                {
                    "attempt_id": attempt_id,
                    "host_alias": "local",
                    "workstream_id": workstream_b,
                    "phase": "running",
                }
            ),
            encoding="ascii",
        )
        try:
            _select_running_attachment(handle, (workstream_a, workstream_a))
        except HarnessBlocked as error:
            assert error.reason == "attachment-workstream-duplicate"
        else:
            raise AssertionError("duplicate fixture Workstream was accepted")
        attachment.write_text(
            json.dumps(
                {
                    "attempt_id": attempt_id,
                    "host_alias": "local",
                    "workstream_id": workstream_b,
                    "phase": "running",
                }
            ),
            encoding="ascii",
        )
        assert (
            _wait_for_running_attachment(
                handle,
                (workstream_a, workstream_b),
                expected_index=1,
                timeout=1,
                sleep=lambda _delay: None,
            )
            == 1
        )
        try:
            _wait_for_running_attachment(
                handle,
                (workstream_a, workstream_b),
                expected_index=0,
                timeout=0.001,
                sleep=lambda _delay: time.sleep(0.002),
            )
        except HarnessBlocked as error:
            assert error.reason == "attachment-workstream-unknown"
        else:
            raise AssertionError("wrong opposite attachment was accepted")

        foreground_calls: list[tuple[tuple[str, ...], bool, float]] = []

        def fake_attach(
            arguments: Sequence[str], environment: Mapping[str, str], timeout: float
        ) -> int:
            foreground_calls.append((tuple(arguments), "TMUX" in environment, timeout))
            return 0

        assert _run_foreground_attach(handle, fake_attach) == 0
        assert foreground_calls == [
            (
                (
                    "tmux",
                    "-S",
                    str(handle.socket),
                    "attach-session",
                    "-t",
                    handle.session,
                ),
                False,
                OPERATOR_ATTACH_SECONDS,
            )
        ]

        utility_baseline = UtilityMetadata(
            "%1",
            42,
            "birth",
            "shell -i",
            "shell",
            Path(temporary) / "project",
            "local",
            workstream_a,
        )
        assert _utility_metadata_difference(utility_baseline, utility_baseline) is None
        for field, reason in (
            ("pane_id", "utility-pane-changed"),
            ("pane_pid", "utility-process-changed"),
            ("process_birth", "utility-process-changed"),
            ("start_command", "utility-command-changed"),
            ("current_command", "utility-command-changed"),
            ("current_path", "utility-cwd-changed"),
            ("host_alias", "utility-context-changed"),
            ("workstream_id", "utility-context-changed"),
        ):
            changed = replace(
                utility_baseline,
                **{
                    field: (
                        "%2"
                        if field == "pane_id"
                        else 43
                        if field == "pane_pid"
                        else "new-birth"
                        if field == "process_birth"
                        else "other-command"
                        if field in {"start_command", "current_command"}
                        else Path(temporary) / "other-project"
                        if field == "current_path"
                        else "other"
                    )
                },
            )
            assert _utility_metadata_difference(utility_baseline, changed) == reason

        client_calls: list[tuple[str, ...]] = []

        def fake_clients(
            arguments: Sequence[str],
        ) -> subprocess.CompletedProcess[str]:
            client_calls.append(tuple(arguments))
            return subprocess.CompletedProcess(
                arguments, 0, f"{handle.session}\n{handle.session}\n", ""
            )

        assert _presentation_client_count(handle, fake_clients) == 2
        assert client_calls == [
            (
                "tmux",
                "-S",
                str(handle.socket),
                "list-clients",
                "-t",
                handle.session,
                "-F",
                PRESENTATION_CLIENT_FORMAT,
            )
        ]
        assert _parse_presentation_client_count("", handle.session) == 0
        try:
            _parse_presentation_client_count("foreign-session\n", handle.session)
        except HarnessBlocked as error:
            assert error.reason == "presentation-client-invalid"
        else:
            raise AssertionError("foreign presentation client was accepted")

        workflow = LocalWorkflowState("setup")
        for phase in LOCAL_PHASES[1:]:
            workflow = _advance_local_phase(workflow, phase)
        assert workflow.phase == "cleanup"
        try:
            _advance_local_phase(LocalWorkflowState("setup"), "shell-exit")
        except HarnessBlocked as error:
            assert error.reason == "internal-error"
        else:
            raise AssertionError("local workflow skipped a checkpoint")

        read_fd, write_fd = os.pipe()
        stop = threading.Event()
        drain = threading.Thread(target=_drain_pty, args=(read_fd, stop), daemon=True)
        drain.start()
        os.write(write_fd, b"discarded")
        os.close(write_fd)
        _stop_pty_drain(NavigatorProcess(None, read_fd, drain, stop, ""))
        assert not drain.is_alive()

        assert _path_under(Path(temporary), str(Path(temporary) / "child"))
        assert not _path_under(Path(temporary), "/var/empty")

        stale_root = create_disposable_root()
        stale_state = stale_root.path / "state"
        stale_directory = stale_state / "presentation" / "presentation-abcdef012345"
        stale_directory.mkdir(parents=True)
        stale_socket_path = stale_directory / "tmux.sock"
        stale_socket = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        stale_socket.bind(str(stale_socket_path))
        stale_socket.close()

        def absent_tmux(_arguments: Sequence[str]) -> subprocess.CompletedProcess[str]:
            return subprocess.CompletedProcess(_arguments, 1, "", "no server running\n")

        stale_cleanup = _cleanup_local(
            Path("candidate"),
            stale_root,
            None,
            None,
            None,
            (),
            "ordinary",
            tmux_runner=absent_tmux,
            ordinary_fingerprint=lambda: "ordinary",
        )
        assert stale_cleanup == ("complete", "complete", True)
        assert not stale_root.path.exists()

        live_root = create_disposable_root()
        live_state = live_root.path / "state"
        live_directory = live_state / "presentation" / "presentation-abcdef012345"
        live_directory.mkdir(parents=True)
        live_socket_path = live_directory / "tmux.sock"
        live_socket = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        live_socket.bind(str(live_socket_path))

        def live_tmux(arguments: Sequence[str]) -> subprocess.CompletedProcess[str]:
            return subprocess.CompletedProcess(arguments, 0, "", "")

        refused_cleanup = _cleanup_local(
            Path("candidate"),
            live_root,
            None,
            None,
            None,
            (),
            "ordinary",
            tmux_runner=live_tmux,
            ordinary_fingerprint=lambda: "ordinary",
        )
        assert refused_cleanup == ("incomplete", "ownership-ambiguous", True)
        live_socket.close()
        assert cleanup_disposable_root(live_root) == "complete"

        other_root = create_disposable_root()
        other_state = other_root.path / "state"
        other_directory = other_state / "presentation" / "presentation-abcdef012345"
        other_directory.mkdir(parents=True)
        other_socket_path = other_directory / "tmux.sock"
        other_socket = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        other_socket.bind(str(other_socket_path))

        def other_tmux(arguments: Sequence[str]) -> subprocess.CompletedProcess[str]:
            return subprocess.CompletedProcess(
                arguments,
                1 if "has-session" in arguments else 0,
                "",
                "session missing\n",
            )

        other_cleanup = _cleanup_local(
            Path("candidate"),
            other_root,
            None,
            None,
            None,
            (),
            "ordinary",
            tmux_runner=other_tmux,
            ordinary_fingerprint=lambda: "ordinary",
        )
        assert other_cleanup == ("incomplete", "ownership-ambiguous", True)
        other_socket.close()
        assert cleanup_disposable_root(other_root) == "complete"

        runtime_identity = RuntimeIdentity(
            Path(temporary) / "runtime.sock",
            "runtime-session",
            "%1",
            123,
            "birth",
            "runtime",
            Path(temporary),
        )
        births = iter(("birth", "birth", None))
        sockets = iter((True, True, False))
        assert (
            _wait_for_runtime_disappearance(
                runtime_identity,
                timeout=1,
                birth_reader=lambda _pid: next(births),
                socket_exists=lambda _path: next(sockets),
                sleep=lambda _delay: None,
            )
            is None
        )
        assert (
            _wait_for_runtime_disappearance(
                runtime_identity,
                timeout=0.001,
                birth_reader=lambda _pid: "birth",
                socket_exists=lambda _path: False,
                sleep=lambda _delay: time.sleep(0.002),
            )
            == "cleanup-process-survived"
        )

    good_two = (
        "%1\tnavigator\t\t\t0\t0\t0\t32\t44\t128\t44\n"
        f"%2\tprovider\tlocal\t{workstream_a}\t0\t32\t0\t96\t44\t128\t44"
    )
    blank_two = (
        "%0\tnavigator\t\t\t0\t0\t0\t32\t24\t128\t24\n"
        "%1\tprovider\t\t\t0\t33\t0\t95\t24\t128\t24"
    )
    _validate_two_pane_shape(parse_topology(blank_two))
    initial_handles: list[PresentationHandle | None] = [None, handle, handle]
    initial_topologies: list[tuple[PaneRecord, ...] | HarnessBlocked] = [
        HarnessBlocked("topology-role-missing"),
        parse_topology(blank_two),
    ]

    def fake_initial_handle(_state_root: Path) -> PresentationHandle | None:
        return initial_handles.pop(0)

    def fake_initial_topology(_handle: PresentationHandle) -> tuple[PaneRecord, ...]:
        value = initial_topologies.pop(0)
        if isinstance(value, HarnessBlocked):
            raise value
        return value

    waited_handle, waited_topology = _wait_for_initial_presentation(
        Path(temporary),
        handle_reader=fake_initial_handle,
        topology_reader=fake_initial_topology,
        timeout=1,
        sleep=lambda _delay: None,
    )
    assert waited_handle == handle
    assert waited_topology == parse_topology(blank_two)
    validate_supported_topology(parse_topology(good_two))
    try:
        _validate_two_pane_shape(
            parse_topology(blank_two.replace("\t33\t0\t95", "\t34\t0\t94"))
        )
    except HarnessBlocked as error:
        assert error.reason == "topology-blank-geometry"
    else:
        raise AssertionError("unsupported blank two-pane geometry was accepted")
    missing_role = blank_two.replace("%1\tprovider", "%1\tnavigator")
    try:
        _validate_two_pane_shape(parse_topology(missing_role))
    except HarnessBlocked as error:
        assert error.reason == "topology-role-missing"
    else:
        raise AssertionError("missing provider role was accepted")
    try:
        parse_topology("not-a-pane")
    except HarnessBlocked as error:
        assert error.reason == "topology-parse-invalid"
    else:
        raise AssertionError("malformed topology was accepted")
    invalid_state = Path(temporary) / "invalid-state"
    (invalid_state / "presentation" / "presentation-not-valid").mkdir(parents=True)
    try:
        _presentation_artifact(invalid_state)
    except HarnessBlocked as error:
        assert error.reason == "presentation-handle-invalid"
    else:
        raise AssertionError("invalid presentation handle was accepted")
    good_three = (
        "%1\tnavigator\t\t\t0\t0\t0\t32\t44\t128\t44\n"
        f"%2\tprovider\tlocal\t{workstream_b}\t0\t32\t0\t96\t22\t128\t44\n"
        f"%3\tutility\tlocal\t{workstream_a}\t0\t32\t22\t96\t22\t128\t44"
    )
    validate_supported_topology(parse_topology(good_three))
    invalid_utility_context = good_three.replace(
        f"%3\tutility\tlocal\t{workstream_a}",
        "%3\tutility\t\t",
    )
    try:
        validate_supported_topology(parse_topology(invalid_utility_context))
    except HarnessBlocked as error:
        assert error.reason == "topology-context-invalid"
    else:
        raise AssertionError("empty utility context was accepted")
    malformed_provider_context = good_three.replace(
        f"%2\tprovider\tlocal\t{workstream_b}",
        "%2\tprovider\tlocal\tbad-workstream",
    )
    try:
        validate_supported_topology(parse_topology(malformed_provider_context))
    except HarnessBlocked as error:
        assert error.reason == "topology-context-invalid"
    else:
        raise AssertionError("malformed provider context was accepted")
    try:
        validate_supported_topology(
            parse_topology(good_three.replace("%3\tutility", "%3\tprovider"))
        )
    except HarnessBlocked as error:
        assert error.reason == "topology-invalid"
    else:
        raise AssertionError("duplicate provider role was accepted")

    try:
        validate_result_privacy({**blocked, "terminal": "secret"})
    except ResultPrivacyError:
        pass
    else:
        raise AssertionError("forbidden result key was accepted")
    path_like = {**blocked, "study": "/secret/provider-root"}
    try:
        validate_result_privacy(path_like)
    except ResultPrivacyError:
        pass
    else:
        raise AssertionError("path-like result value was accepted")
    raw = {
        **blocked,
        "tool_versions": {
            "ssh": "provider-output",
            "tmux": "not-run",
            "wsnav": "not-run",
        },
    }
    try:
        validate_result_privacy(raw)
    except ResultPrivacyError:
        pass
    else:
        raise AssertionError("raw result value was accepted")
    oversized = {**blocked, "reason": "x" * (MAX_REASON_LENGTH + 1)}
    try:
        validate_result_privacy(oversized)
    except ResultPrivacyError:
        pass
    else:
        raise AssertionError("oversized reason was accepted")
    assert bounded_category(HarnessBlocked("not-implemented")) == "not-implemented"
    assert bounded_category("/secret/provider-output") == "internal-error"

    calls: list[tuple[tuple[str, ...], bool]] = []

    def fake_runner(
        arguments: Sequence[str], environment: Mapping[str, str], _timeout: float
    ) -> subprocess.CompletedProcess[str]:
        calls.append((tuple(arguments), "TMUX" in environment))
        return subprocess.CompletedProcess(arguments, 0, "one:two:three\n", "")

    first = ordinary_tmux_fingerprint(fake_runner)
    second = ordinary_tmux_fingerprint(fake_runner)
    assert first == second and len(first) == 64
    assert calls and all(not had_tmux for _, had_tmux in calls)
    assert all("kill" not in arguments for arguments, _ in calls)

    ready_assertions = default_assertions()
    for name in (
        "abi2_preflight",
        "cleanup_complete",
        "local_below_provider_geometry",
        "local_canonical_cwd",
        "local_detach_reattach",
        "local_git_status",
        "local_guarded_close",
        "local_one_shell_idempotent",
        "local_provider_interactive",
        "local_running_attachment",
        "local_runtime_identity",
        "local_shell_exit_cleanup",
        "local_topology",
        "local_utility_context_fixed",
        "ordinary_tmux_unchanged",
        "privacy_bounded",
    ):
        ready_assertions[name] = True
    ready_local = make_result(
        status="blocked",
        reason="visual-confirmation-required",
        operator_confirmed=True,
        assertions=ready_assertions,
        cleanup_status="complete",
        cleanup_reason="complete",
    )
    assert _local_machine_ready_for_remote(ready_local)
    remote_assertions = default_assertions()
    remote_assertions.update(
        {
            "abi2_preflight": True,
            "cleanup_complete": True,
            "ordinary_tmux_unchanged": True,
            "privacy_bounded": True,
        }
    )
    for name in REMOTE_MACHINE_ASSERTIONS:
        remote_assertions[name] = True
    ready_remote = make_result(
        status="blocked",
        reason="visual-confirmation-required",
        operator_confirmed=True,
        assertions=remote_assertions,
        tool_versions={"ssh": "checked", "tmux": "checked", "wsnav": "checked"},
        cleanup_status="complete",
        cleanup_reason="complete",
    )
    combined = _combine_acceptance_results(ready_local, ready_remote)
    assert combined["status"] == "blocked"
    assert combined["reason"] == "visual-confirmation-required"
    assert combined["primary_reason"] == "visual-confirmation-required"
    assert combined["assertions"]["local_topology"]
    assert combined["assertions"]["ssh_topology"]
    assert combined["cleanup"]["status"] == "complete"


def emit_result(result: Mapping[str, Any], path: Path | None) -> None:
    validate_result_privacy(result)
    if path is None:
        sys.stdout.write(json.dumps(result, indent=2, sort_keys=True) + "\n")
    else:
        write_result(path, result)


def main(argv: Sequence[str] | None = None) -> int:
    options = parse_arguments(argv)
    if options.self_test:
        run_self_test()
        print("d12 presentation harness self-test passed")
        return 0
    if not options.confirm_live:
        result = make_result(
            status="blocked",
            reason="operator-confirmation-required",
            operator_confirmed=False,
        )
    else:
        workspace = Path(__file__).resolve().parents[1]
        try:
            candidate = resolve_candidate_binary(workspace)
            local_result = _run_local_acceptance(candidate)
            if _local_machine_ready_for_remote(local_result):
                result = _combine_acceptance_results(
                    local_result, _run_remote_acceptance(candidate)
                )
            else:
                result = local_result
        except HarnessBlocked as error:
            result = make_result(
                status="blocked", reason=error.reason, operator_confirmed=True
            )
        except Exception as error:  # noqa: BLE001 - sanitize live setup failures.
            result = make_result(
                status="blocked",
                reason=bounded_category(error),
                operator_confirmed=True,
            )
    emit_result(result, options.result)
    return 0 if result["status"] == "pass" else 1


if __name__ == "__main__":
    raise SystemExit(main())
