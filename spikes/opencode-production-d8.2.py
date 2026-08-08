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
import signal
import socket
import sqlite3
import subprocess
import sys
import tempfile
import time
from collections.abc import Callable
from dataclasses import dataclass
from functools import partial
from pathlib import Path
from typing import Any

from opencode_support import environment_for_directory, isolated_environment

MODEL = "opencode-go/deepseek-v4-flash"
MARKER = "WSNAV_D82_ACCEPTANCE_RESULT"
MAX_GROUP_MEMBERS = 128
MAX_CLEANUP_DIAGNOSTICS = 8
MAX_DIAGNOSTIC_LENGTH = 192
MAX_REASON_LENGTH = 512
FINAL_CLEANUP_RETRY_SECONDS = 5
ROOT_REFERENCE_WAIT_SECONDS = 5
ROOT_REMOVAL_WAIT_SECONDS = 5
FORK_STABILITY_TIMEOUT_SECONDS = 5
FORK_STABLE_WINDOW_SECONDS = 0.75
FORK_STABILITY_POLL_SECONDS = 0.1
MAX_FORK_REVISION_RETRIES = 1
REVISION_CONFLICT_DIAGNOSTIC = (
    "error: host rejected the request: revision conflict; refresh this host"
)
PROMPT = (
    f"Reply with the exact token {MARKER} and nothing else. "
    "Do not use tools, inspect files, or make changes."
)


class AcceptanceFailure(RuntimeError):
    """A bounded product assertion failed."""


class AcceptanceBlocked(RuntimeError):
    """A required local acceptance prerequisite was unavailable."""


@dataclass(frozen=True)
class RootReference:
    """Sanitized evidence that a process still references a disposable root.

    The acceptance result must never include command lines, environments,
    mapped paths, descriptor targets, or provider payloads.  Numeric process
    identity plus a small fixed category is enough for an operator to
    distinguish a surviving culprit without disclosing that content.
    """

    pid: int | None
    birth: str | None
    process_group: int | None
    session: int | None
    category: str


@dataclass(frozen=True)
class ForkEffectBaseline:
    """Counts used to prove a rejected Fork created no durable effect."""

    compound_operation_count: int
    destination_workstream_count: int


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


_SOURCE_RUNTIME_IDENTITY_FIELDS = (
    "provider",
    "runtime_id",
    "session_id",
    "handle_generation",
    "tmux_generation",
)


def _settled_source_sample(
    info: dict[str, Any] | None,
) -> tuple[int, str, tuple[str, ...]] | None:
    """Return the bounded source boundary used by a Fork attempt."""

    if info is None or info.get("provider") != "opencode":
        return None
    try:
        raw_revision = info["revision"]
    except KeyError:
        return None
    if type(raw_revision) is not int:
        return None
    revision = raw_revision
    if revision <= 0:
        return None
    raw_settled_id = info.get("settled_id")
    if not isinstance(raw_settled_id, str) or not raw_settled_id.strip():
        return None
    identity: list[str] = []
    for field in _SOURCE_RUNTIME_IDENTITY_FIELDS:
        value = info.get(field)
        if not isinstance(value, str) or not value.strip():
            return None
        identity.append(value)
    return (
        revision,
        raw_settled_id,
        tuple(identity),
    )


def wait_for_stable_settled_source(
    state_root: Path,
    workstream_id: str,
    *,
    baseline: dict[str, Any] | None = None,
    timeout: float = FORK_STABILITY_TIMEOUT_SECONDS,
    stable_window: float = FORK_STABLE_WINDOW_SECONDS,
    poll_interval: float = FORK_STABILITY_POLL_SECONDS,
    read: Callable[[Path, str], dict[str, Any] | None] | None = None,
    clock: Callable[[], float] = time.monotonic,
    sleep: Callable[[float], None] = time.sleep,
) -> dict[str, Any]:
    """Wait for a settled revision/boundary tuple to stop changing.

    ``baseline`` binds the observation to the Runtime and settled provider
    boundary that the caller already displayed. A newer revision on that same
    boundary is safe to use after it settles; a new boundary or Runtime is
    not, because it could silently Fork a later turn or replacement Runtime.
    """

    reader = runtime_info if read is None else read
    initial = baseline if baseline is not None else reader(state_root, workstream_id)
    expected = _settled_source_sample(initial)
    if expected is None:
        raise AcceptanceFailure("observer-settled-boundary-unavailable")
    expected_revision = expected[0]
    expected_boundary = expected[1]
    expected_runtime = expected[2]
    deadline = clock() + timeout
    candidate: tuple[int, str] | None = None
    stable_since: float | None = None
    while True:
        current = reader(state_root, workstream_id)
        sample = _settled_source_sample(current)
        now = clock()
        if sample is not None:
            revision, settled_id, runtime_identity = sample
            if revision < expected_revision:
                raise AcceptanceFailure("observer-revision-regressed")
            if settled_id != expected_boundary:
                raise AcceptanceFailure("observer-settled-boundary-changed")
            if runtime_identity != expected_runtime:
                raise AcceptanceFailure("observer-runtime-changed")
            current_tuple = (revision, settled_id)
            if candidate != current_tuple:
                candidate = current_tuple
                stable_since = now
            elif stable_since is not None and now - stable_since >= stable_window:
                # ``current`` is known non-None whenever ``sample`` is set.
                return current  # type: ignore[return-value]
        else:
            candidate = None
            stable_since = None
        if now >= deadline:
            raise AcceptanceFailure("observer-revision-churn")
        sleep(min(poll_interval, max(0.0, deadline - now)))


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


def bounded_wsnav_failure(result: subprocess.CompletedProcess[str], label: str) -> str:
    """Classify one command failure without copying raw remote diagnostics."""

    categories = (
        ("revision conflict; refresh this host", "revision-conflict"),
        ("workstream outcome needs recovery", "recovery-required"),
        ("OpenCode Fork response was lost", "external-effect-unknown"),
        ("fork source is no longer available", "source-unavailable"),
        ("workstream creation is unavailable", "creation-unavailable"),
        ("host command timed out", "timeout"),
        (
            "host command failed without a usable protocol response",
            "protocol-unavailable",
        ),
    )
    category = next(
        (category for fragment, category in categories if fragment in result.stderr),
        "other",
    )
    return f"wsnav-command-failed:{label}:{category}"


def is_pre_effect_revision_conflict(
    result: subprocess.CompletedProcess[str],
) -> bool:
    """Recognize only the host rejection that precedes Fork creation."""

    return (
        result.returncode != 0
        and not (result.stdout or "").strip()
        and (result.stderr or "").strip() == REVISION_CONFLICT_DIAGNOSTIC
    )


def invoke_fork_with_revision_retry(
    source_id: str,
    source: dict[str, Any],
    *,
    invoke: Callable[[str, str], subprocess.CompletedProcess[str]],
    refresh: Callable[[dict[str, Any]], dict[str, Any]],
    assert_no_effect: Callable[[], None],
    label: str = "host-fork",
    max_retries: int = MAX_FORK_REVISION_RETRIES,
) -> subprocess.CompletedProcess[str]:
    """Retry one pre-effect revision rejection against the same boundary."""

    baseline = _settled_source_sample(source)
    if baseline is None:
        raise AcceptanceFailure("observer-settled-boundary-unavailable")
    expected_revision = str(baseline[0])
    retry_budget = min(max(max_retries, 0), MAX_FORK_REVISION_RETRIES)
    for attempt in range(retry_budget + 1):
        result = invoke(source_id, expected_revision)
        if result.returncode == 0:
            return result
        if not is_pre_effect_revision_conflict(result):
            raise AcceptanceFailure(bounded_wsnav_failure(result, label))
        if attempt >= retry_budget:
            raise AcceptanceFailure("observer-revision-churn")
        assert_no_effect()
        refreshed = refresh(source)
        current = _settled_source_sample(refreshed)
        if current is None:
            raise AcceptanceFailure("observer-settled-boundary-unavailable")
        if current[1] != baseline[1]:
            raise AcceptanceFailure("observer-settled-boundary-changed")
        if current[2] != baseline[2]:
            raise AcceptanceFailure("observer-runtime-changed")
        if current[0] < baseline[0]:
            raise AcceptanceFailure("observer-revision-regressed")
        if current[0] == int(expected_revision):
            raise AcceptanceFailure("observer-revision-churn")
        expected_revision = str(current[0])
    raise AcceptanceFailure(f"wsnav-command-failed:{label}:revision-conflict")


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
                      r.tmux_session, r.provider_pid, r.process_birth,
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
        "tmux_session",
        "provider_pid",
        "provider_birth",
        "session_id",
        "settled_id",
        "handle_generation",
        "port",
        "observer_pid",
        "observer_birth",
        "observer_status",
    )
    return dict(zip(keys, row, strict=True))


def fork_effect_baseline(
    state_root: Path, source_workstream_id: str
) -> ForkEffectBaseline:
    """Read only row counts needed to prove a rejected Fork had no effect."""

    database = state_root / "host.sqlite"
    if not database.exists():
        raise AcceptanceFailure("fork-effect-baseline-unavailable")
    try:
        with sqlite3.connect(database) as connection:
            row = connection.execute(
                """SELECT
                       (SELECT COUNT(*) FROM compound_operations),
                       (SELECT COUNT(*) FROM workstreams
                          WHERE source_workstream_id = ?)""",
                (source_workstream_id,),
            ).fetchone()
    except sqlite3.Error as error:
        raise AcceptanceFailure("fork-effect-baseline-unavailable") from error
    if row is None or len(row) != 2:
        raise AcceptanceFailure("fork-effect-baseline-unavailable")
    operation_count, destination_count = row
    if (
        not isinstance(operation_count, int)
        or not isinstance(destination_count, int)
        or operation_count < 0
        or destination_count < 0
    ):
        raise AcceptanceFailure("fork-effect-baseline-unavailable")
    return ForkEffectBaseline(operation_count, destination_count)


def assert_fork_effect_unchanged(
    state_root: Path,
    source_workstream_id: str,
    baseline: ForkEffectBaseline,
) -> None:
    """Reject a retry if the rejected command changed durable Fork state."""

    current = fork_effect_baseline(state_root, source_workstream_id)
    if (
        current.compound_operation_count != baseline.compound_operation_count
        or current.destination_workstream_count != baseline.destination_workstream_count
    ):
        raise AcceptanceFailure("fork-effect-observed")


def ready_runtime(state_root: Path, workstream_id: str) -> dict[str, Any] | None:
    info = runtime_info(state_root, workstream_id)
    if (
        info is not None
        and info["provider"] == "opencode"
        and info["provider_pid"]
        and info["provider_birth"]
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


def private_pane_identity(state_root: Path, runtime: dict[str, Any]) -> ProcessIdentity:
    socket_path = private_socket(state_root, str(runtime["runtime_id"]))
    session = str(runtime.get("tmux_session") or "")
    if not socket_path.exists() or not session:
        raise AcceptanceFailure("cleanup-session-corroboration-missing")
    environment = os.environ.copy()
    environment.pop("TMUX", None)
    result = run(
        [
            "tmux",
            "-S",
            str(socket_path),
            "display-message",
            "-p",
            "-t",
            f"{session}:0.0",
            "#{pane_pid}",
        ],
        env=environment,
        timeout=5,
        check=False,
    )
    value = result.stdout.strip()
    if result.returncode != 0 or not value.isdecimal() or "\n" in value:
        raise AcceptanceFailure("cleanup-session-corroboration-missing")
    pane = read_process_identity(int(value))
    if pane is None:
        raise AcceptanceFailure("cleanup-session-corroboration-missing")
    return pane


def capture_provider_evidence(
    state_root: Path, runtime: dict[str, Any]
) -> ProviderEvidence | None:
    raw_pid = runtime.get("provider_pid")
    raw_birth = runtime.get("provider_birth")
    if raw_pid is None and raw_birth is None:
        return None
    if raw_pid is None or not raw_birth:
        raise AcceptanceFailure("cleanup-provider-identity-ambiguous")
    try:
        pid = int(raw_pid)
    except (TypeError, ValueError) as error:
        raise AcceptanceFailure("cleanup-provider-identity-ambiguous") from error
    current = read_process_identity(pid)
    if current is None:
        # A live private pane with a missing recorded PID is an ownership
        # ambiguity, not evidence that the process tree is clean.
        if private_socket(state_root, str(runtime["runtime_id"])).exists():
            private_pane_identity(state_root, runtime)
            raise AcceptanceFailure("cleanup-provider-identity-lost")
        return None
    expected_birth = str(raw_birth)
    if current.birth != expected_birth:
        raise AcceptanceFailure("cleanup-provider-identity-reused")
    if current.process_group != pid:
        raise AcceptanceFailure("cleanup-provider-group-ambiguous")
    pane = private_pane_identity(state_root, runtime)
    if (
        pane.pid != pid
        or pane.birth != expected_birth
        or pane.session != current.session
    ):
        raise AcceptanceFailure("cleanup-session-corroboration-mismatch")
    members = process_group_members(current.process_group)
    if not any(
        member.pid == pid and member.birth == expected_birth for member in members
    ) or any(member.session != current.session for member in members):
        raise AcceptanceFailure("cleanup-session-corroboration-mismatch")
    return ProviderEvidence(
        pid=pid,
        birth=expected_birth,
        process_group=current.process_group,
        session=current.session,
        pane_pid=pane.pid,
        pane_birth=pane.birth,
        pane_session=pane.session,
        members=members,
    )


def process_group_members(process_group: int) -> tuple[ProcessIdentity, ...]:
    if process_group <= 0:
        raise AcceptanceFailure("cleanup-provider-group-ambiguous")
    try:
        processes = tuple(Path("/proc").iterdir())
    except OSError as error:
        raise AcceptanceFailure("cleanup-provider-group-ambiguous") from error
    members: list[ProcessIdentity] = []
    for process in processes:
        if not process.name.isdecimal():
            continue
        identity = read_process_identity(int(process.name))
        if identity is not None and identity.process_group == process_group:
            members.append(identity)
            if len(members) > MAX_GROUP_MEMBERS:
                raise AcceptanceFailure("cleanup-provider-group-unbounded")
    return tuple(members)


def prove_provider_group(
    evidence: ProviderEvidence,
) -> tuple[ProcessIdentity, tuple[ProcessIdentity, ...]] | None:
    """Re-prove the recorded leader before any group signal is sent."""

    current = read_process_identity(evidence.pid)
    if current is None:
        return None
    if (
        current.birth != evidence.birth
        or current.process_group != evidence.pid
        or current.process_group != evidence.process_group
        or current.session != evidence.session
        or current.state == "Z"
        or evidence.pane_pid != evidence.pid
        or evidence.pane_birth != evidence.birth
        or evidence.pane_session != evidence.session
    ):
        return None
    members = process_group_members(evidence.process_group)
    if any(member.session != evidence.session for member in members):
        return None
    if not any(member.pid == evidence.pid for member in members):
        return None
    return current, members


def _group_is_quiet(evidence: ProviderEvidence) -> bool:
    members = process_group_members(evidence.process_group)
    return not any(member.state != "Z" for member in members)


def signal_provider_members(
    evidence: ProviderEvidence,
    members: tuple[ProcessIdentity, ...],
    kind: signal.Signals,
    *,
    require_leader: bool,
) -> None:
    """Signal an already-proven member snapshot through pidfds.

    The KILL phase may run after a TERM-exiting group leader is gone. Every
    remaining member must still match the original PID birth, PGID, and
    session; an unknown or reused member aborts before any descriptor is
    signalled.
    """

    if not hasattr(os, "pidfd_open") or not hasattr(signal, "pidfd_send_signal"):
        raise AcceptanceFailure("cleanup-pidfd-unavailable")
    if require_leader and prove_provider_group(evidence) is None:
        raise AcceptanceFailure("cleanup-provider-group-identity-lost")
    current_members = process_group_members(evidence.process_group)
    snapshot = {member.pid: member for member in members}
    current_live = {
        member.pid: member for member in current_members if member.state != "Z"
    }
    unknown = set(current_live) - set(snapshot)
    if unknown:
        raise AcceptanceFailure("cleanup-provider-group-identity-lost")
    descriptors: list[int] = []
    try:
        for member in current_live.values():
            expected = snapshot[member.pid]
            if member.birth != expected.birth:
                raise AcceptanceFailure("cleanup-provider-group-identity-lost")
            if member.state == "Z":
                continue
            try:
                descriptor = os.pidfd_open(member.pid)
            except OSError as error:
                raise AcceptanceFailure(
                    "cleanup-provider-group-signal-failed"
                ) from error
            current = read_process_identity(member.pid)
            if (
                current is None
                or current.birth != expected.birth
                or current.process_group != evidence.process_group
                or current.session != evidence.session
            ):
                os.close(descriptor)
                raise AcceptanceFailure("cleanup-provider-group-identity-lost")
            descriptors.append(descriptor)
        if not descriptors:
            return
        for descriptor in descriptors:
            try:
                signal.pidfd_send_signal(descriptor, kind)
            except ProcessLookupError:
                continue
            except OSError as error:
                raise AcceptanceFailure(
                    "cleanup-provider-group-signal-failed"
                ) from error
    finally:
        for descriptor in descriptors:
            os.close(descriptor)


def signal_proven_provider_group(
    evidence: ProviderEvidence,
    members: tuple[ProcessIdentity, ...],
    kind: signal.Signals,
) -> None:
    """Signal an exact leader-backed group snapshot through pidfds."""

    signal_provider_members(evidence, members, kind, require_leader=True)


def cleanup_provider_group(
    evidence: ProviderEvidence | None,
    reference_root: Path,
    *,
    check_root: bool = True,
) -> None:
    """Terminate only a currently re-proven provider process group.

    A missing or reused leader is never signalled. If descendants survive
    after the leader disappears, cleanup is falsified and the root is kept for
    operator inspection.
    """

    if evidence is None:
        if check_root:
            reference = process_references_root(reference_root)
            if reference is not None:
                raise AcceptanceFailure(format_root_reference(reference))
        return
    if evidence.process_group != evidence.pid:
        raise AcceptanceFailure("cleanup-provider-group-ambiguous")
    proven = prove_provider_group(evidence)
    if proven is None:
        if _group_is_quiet(evidence):
            return
        raise AcceptanceFailure("cleanup-provider-group-identity-lost")
    _, original_members = proven
    if not _group_is_quiet(evidence):
        signal_proven_provider_group(evidence, original_members, signal.SIGTERM)
        try:
            wait_for(
                lambda: _group_is_quiet(evidence),
                "cleanup-provider-group-quiet",
                timeout=5,
            )
        except AcceptanceFailure:
            # The leader may have exited while an exact child ignored TERM.
            # KILL is still bounded to the original member birth/PGID/session
            # snapshot and uses pidfds, so it does not need a live leader.
            signal_provider_members(
                evidence,
                original_members,
                signal.SIGKILL,
                require_leader=False,
            )
            wait_for(
                lambda: _group_is_quiet(evidence),
                "cleanup-provider-group-quiet-after-kill",
                timeout=5,
            )
    if not _group_is_quiet(evidence):
        raise AcceptanceFailure("cleanup-provider-group-survived")
    if check_root:
        reference = process_references_root(reference_root)
        if reference is not None:
            raise AcceptanceFailure(format_root_reference(reference))


def kill_private_runtime(state_root: Path, runtime_id: str) -> None:
    environment = os.environ.copy()
    environment.pop("TMUX", None)
    run(
        ["tmux", "-S", str(private_socket(state_root, runtime_id)), "kill-server"],
        env=environment,
    )


@dataclass(frozen=True)
class ProcessIdentity:
    """Small, disposable /proc identity used only by harness cleanup."""

    pid: int
    birth: str
    parent: int
    process_group: int
    session: int
    state: str


@dataclass(frozen=True)
class ProviderEvidence:
    """Provider identity plus private-pane/session corroboration."""

    pid: int
    birth: str
    process_group: int
    session: int
    pane_pid: int
    pane_birth: str
    pane_session: int
    members: tuple[ProcessIdentity, ...]


def read_process_identity(pid: int) -> ProcessIdentity | None:
    if pid <= 0:
        raise AcceptanceFailure("provider-process-probe-ambiguous")
    try:
        stat = Path(f"/proc/{pid}/stat").read_text(encoding="utf-8")
    except FileNotFoundError:
        return None
    except OSError as error:
        raise AcceptanceFailure("provider-process-probe-ambiguous") from error
    close_paren = stat.rfind(")")
    if close_paren < 0:
        raise AcceptanceFailure("provider-process-probe-ambiguous")
    fields = stat[close_paren + 2 :].split()
    if len(fields) <= 19:
        raise AcceptanceFailure("provider-process-probe-ambiguous")
    try:
        return ProcessIdentity(
            pid=pid,
            state=fields[0],
            parent=int(fields[1]),
            process_group=int(fields[2]),
            session=int(fields[3]),
            birth=fields[19],
        )
    except ValueError as error:
        raise AcceptanceFailure("provider-process-probe-ambiguous") from error


def pid_is_gone(pid: int) -> bool:
    identity = read_process_identity(pid)
    return identity is None or identity.state == "Z"


def process_birth(pid: int) -> str | None:
    identity = read_process_identity(pid)
    return identity.birth if identity is not None else None


def process_identity_is_gone(pid: int, expected_birth: str) -> bool:
    identity = read_process_identity(pid)
    return identity is None or identity.birth != expected_birth or identity.state == "Z"


def _path_is_under(path: str, root: str) -> bool:
    path = path.removesuffix(" (deleted)")
    return path == root or path.startswith(f"{root}{os.sep}")


def _bytes_contain_root(value: bytes, root: bytes) -> bool:
    """Find a root argument without treating a similarly-prefixed path as ours."""

    start = 0
    while (offset := value.find(root, start)) >= 0:
        before = value[offset - 1] if offset else None
        after_offset = offset + len(root)
        after = value[after_offset] if after_offset < len(value) else None
        boundary_before = before is None or before in b"\x00 \t\n=:'\""
        boundary_after = after is None or after in b"\x00 /\\:'\""
        if boundary_before and boundary_after:
            return True
        start = offset + 1
    return False


def _read_bounded(path: Path, limit: int) -> bytes:
    with path.open("rb") as stream:
        return stream.read(limit)


def _root_reference(
    process: Path,
    category: str,
    before: ProcessIdentity | None,
) -> RootReference:
    """Build bounded process identity for a root-reference diagnostic."""

    try:
        pid = int(process.name)
    except ValueError:
        return RootReference(None, None, None, None, "proc-scan-ambiguous")
    try:
        identity = read_process_identity(pid)
    except AcceptanceFailure:
        # The reference itself is enough to fail closed.  Do not copy any
        # malformed /proc content into the operator result.
        return RootReference(pid, None, None, None, "identity-ambiguous")
    if identity is None:
        return RootReference(pid, None, None, None, "identity-ambiguous")
    if (
        before is None
        or identity.birth != before.birth
        or identity.process_group != before.process_group
        or identity.session != before.session
    ):
        return RootReference(pid, None, None, None, "identity-ambiguous")
    return RootReference(
        identity.pid,
        identity.birth,
        identity.process_group,
        identity.session,
        category,
    )


def format_root_reference(reference: RootReference) -> str:
    """Render only bounded, sanitized identity/category cleanup evidence."""

    category = reference.category
    if len(category) > 48 or not category.replace("-", "").isalnum():
        category = "unknown"
    fields = [f"cleanup-root-reference-present:{category}"]
    if reference.pid is not None and reference.pid > 0:
        fields.append(f"pid={reference.pid}")
    if reference.birth is not None and reference.birth.isdecimal():
        fields.append(f"birth={reference.birth[:32]}")
    if reference.process_group is not None and reference.process_group > 0:
        fields.append(f"pgrp={reference.process_group}")
    if reference.session is not None and reference.session > 0:
        fields.append(f"session={reference.session}")
    return ":".join(fields)[:MAX_DIAGNOSTIC_LENGTH]


def process_references_root(root: Path) -> RootReference | None:
    """Scan command, cwd, environment, maps, and descriptors before removal.

    A disappearing or permission-limited process is not evidence that it owns
    this user-owned disposable root. Readable command, cwd, environment, map,
    and descriptor references are all checked; an unreadable process is left
    for the explicit group-identity proof to reject if it is our provider.
    Return only sanitized process identity and a fixed reference category;
    never return the matching content or target path.
    """

    root_text = str(root.absolute())
    root_bytes = os.fsencode(root_text)
    try:
        processes = tuple(Path("/proc").iterdir())
    except OSError:
        return RootReference(None, None, None, None, "proc-scan-unavailable")
    for process in processes:
        if not process.name.isdecimal():
            continue
        # The cleanup authority may legitimately hold its own disposable
        # SQLite/file descriptors while proving that external effects are
        # gone.  Exclude only this exact PID; a parent, sibling, or shared
        # process group remains evidence and is never implicitly trusted.
        if int(process.name) == os.getpid():
            continue
        proc_root = process
        try:
            before = read_process_identity(int(process.name))
        except AcceptanceFailure:
            before = None
        try:
            if _bytes_contain_root(
                _read_bounded(proc_root / "cmdline", 256 * 1024), root_bytes
            ):
                return _root_reference(proc_root, "cmdline", before)
            if _bytes_contain_root(
                _read_bounded(proc_root / "environ", 256 * 1024), root_bytes
            ):
                return _root_reference(proc_root, "environment", before)
            if _bytes_contain_root(
                _read_bounded(proc_root / "maps", 4 * 1024 * 1024), root_bytes
            ):
                return _root_reference(proc_root, "maps", before)
        except FileNotFoundError:
            continue
        except PermissionError:
            continue
        except OSError:
            continue
        for name in ("cwd", "root", "exe"):
            try:
                if _path_is_under(os.readlink(proc_root / name), root_text):
                    return _root_reference(proc_root, name, before)
            except FileNotFoundError:
                break
            except PermissionError:
                continue
            except OSError:
                continue
        descriptors = proc_root / "fd"
        try:
            for descriptor in descriptors.iterdir():
                try:
                    if _path_is_under(os.readlink(descriptor), root_text):
                        return _root_reference(proc_root, "fd", before)
                except FileNotFoundError:
                    continue
                except PermissionError:
                    continue
                except OSError:
                    continue
        except FileNotFoundError:
            continue
        except PermissionError:
            continue
        except OSError:
            continue
    return None


def wait_for_root_quiet(
    root: Path, *, timeout: float = ROOT_REFERENCE_WAIT_SECONDS
) -> RootReference | None:
    """Bound the final teardown wait without changing Park's strict check.

    This is intentionally used only after every registered cleanup target has
    run.  A provider that survived an individual Park assertion remains a
    cleanup diagnostic even if it exits during this final bounded wait.
    """

    deadline = time.monotonic() + timeout
    reference = process_references_root(root)
    while reference is not None and time.monotonic() < deadline:
        time.sleep(0.25)
        reference = process_references_root(root)
    return reference


def wait_for_root_removed(
    root: Path, *, timeout: float = ROOT_REMOVAL_WAIT_SECONDS
) -> RootReference | None:
    """Check that final root removal remains stable for a bounded interval."""

    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        reference = process_references_root(root)
        if root.exists():
            return RootReference(None, None, None, None, "root-remains")
        if reference is not None:
            return reference
        time.sleep(0.25)
    if root.exists():
        return RootReference(None, None, None, None, "root-remains")
    return process_references_root(root)


def bounded_cleanup_reason(error: BaseException) -> str:
    """Keep cleanup diagnostics bounded and free of paths/payloads."""

    if isinstance(error, AcceptanceFailure):
        value = str(error)
        if (
            value
            and len(value) <= MAX_DIAGNOSTIC_LENGTH
            and all(character.isalnum() or character in ":_=-" for character in value)
        ):
            return value
    if isinstance(error, subprocess.TimeoutExpired):
        return "cleanup-error:TimeoutExpired"
    if isinstance(error, sqlite3.Error):
        return "cleanup-error:sqlite"
    if isinstance(error, OSError):
        return "cleanup-error:OSError"
    return f"cleanup-error:{type(error).__name__}"[:MAX_DIAGNOSTIC_LENGTH]


def record_cleanup_diagnostic(diagnostics: list[str], value: str) -> None:
    """Append one deduplicated bounded cleanup diagnostic."""

    if value not in diagnostics and len(diagnostics) < MAX_CLEANUP_DIAGNOSTICS:
        diagnostics.append(value[:MAX_DIAGNOSTIC_LENGTH])


def compose_cleanup_reason(primary_reason: str, diagnostics: list[str]) -> str:
    """Retain the primary outcome while appending bounded cleanup evidence."""

    details = ",".join(diagnostics) or "unknown"
    return f"{primary_reason};cleanup-incomplete={details}"[:MAX_REASON_LENGTH]


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


def assert_provider_group_stopped(
    evidence: ProviderEvidence | None,
    reference_root: Path,
    *,
    check_root: bool = True,
) -> None:
    """Read-only post-action check for every captured provider member."""

    if evidence is None:
        if check_root:
            reference = process_references_root(reference_root)
            if reference is not None:
                raise AcceptanceFailure(format_root_reference(reference))
        return
    for member in evidence.members:
        current = read_process_identity(member.pid)
        if current is None or current.state == "Z":
            continue
        if current.birth != member.birth:
            raise AcceptanceFailure("cleanup-provider-identity-reused")
        raise AcceptanceFailure("cleanup-provider-member-survived")
    remaining = process_group_members(evidence.process_group)
    if any(
        member.state != "Z" and member.session == evidence.session
        for member in remaining
    ):
        raise AcceptanceFailure("cleanup-provider-group-survived")
    if check_root:
        reference = process_references_root(reference_root)
        if reference is not None:
            raise AcceptanceFailure(format_root_reference(reference))


def park_with_observation(
    state_root: Path,
    runtime: dict[str, Any],
    action: Callable[[], subprocess.CompletedProcess[str]],
    reference_root: Path,
    *,
    on_capture: Callable[[ProviderEvidence | None], None] | None = None,
    check_root: bool = True,
) -> subprocess.CompletedProcess[str]:
    """Run the product park/recovery action, then validate without signalling."""

    evidence = capture_provider_evidence(state_root, runtime)
    if on_capture is not None:
        on_capture(evidence)
    result = action()
    assert_provider_group_stopped(evidence, reference_root, check_root=check_root)
    return result


def park_direct(
    binary: Path,
    state_root: Path,
    workstream_id: str,
    env: dict[str, str],
    *,
    reference_root: Path | None = None,
    fallback_evidence: ProviderEvidence | None = None,
    check_root: bool = True,
) -> None:
    before = runtime_info(state_root, workstream_id)
    if before is None:
        return
    cleanup_root = reference_root or state_root.parent
    fallback_matches = (
        fallback_evidence is not None
        and before.get("provider_pid") is not None
        and before.get("provider_birth") is not None
        and int(before["provider_pid"]) == fallback_evidence.pid
        and str(before["provider_birth"]) == fallback_evidence.birth
    )
    evidence = (
        fallback_evidence
        if fallback_matches
        else capture_provider_evidence(state_root, before)
    )
    # Provider cleanup precedes tmux/root removal. This is deliberately done
    # even for a Runtime already marked stopped: lifecycle is state evidence,
    # not proof that an escaped native process exited.
    cleanup_provider_group(evidence, cleanup_root, check_root=check_root)
    socket_path = private_socket(state_root, str(before["runtime_id"]))
    runtime_directory = socket_path.parent
    if (
        before["runtime_lifecycle"] != "stopped"
        or socket_path.exists()
        or runtime_directory.exists()
    ):
        wsnav(binary, state_root, "park", workstream_id, env=env)
    if socket_path.exists() or runtime_directory.exists():
        raise AcceptanceFailure("cleanup-private-runtime-artifacts-present")
    if check_root:
        reference = process_references_root(cleanup_root)
        if reference is not None:
            raise AcceptanceFailure(format_root_reference(reference))


def retry_cleanup_action(
    action: Callable[[], None],
    *,
    timeout: float = FINAL_CLEANUP_RETRY_SECONDS,
    poll_interval: float = 0.25,
) -> None:
    """Retry only final disposable cleanup across transient Runtime races."""

    deadline = time.monotonic() + timeout
    while True:
        try:
            action()
            return
        except (
            AcceptanceFailure,
            OSError,
            sqlite3.Error,
            subprocess.TimeoutExpired,
        ) as error:
            if time.monotonic() >= deadline:
                raise AcceptanceFailure(bounded_cleanup_reason(error)) from error
            time.sleep(poll_interval)


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
    remember: Callable[[Path, str, ProviderEvidence | None], None] | None = None,
    fork_invoke: Callable[[str, dict[str, Any]], subprocess.CompletedProcess[str]]
    | None = None,
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
    if fork_invoke is not None:
        source = wait_for_stable_settled_source(
            host_state,
            source_id,
            baseline=source,
        )

    forked = (
        fork_invoke(source_id, source)
        if fork_invoke is not None
        else invoke(["fork", source_id, str(source["revision"])])
    )
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

    park_with_observation(
        host_state,
        destination,
        lambda: invoke(["park", destination_id, str(destination["revision"])]),
        host_state.parent,
        on_capture=(
            (lambda evidence: remember(host_state, destination_id, evidence))
            if remember is not None
            else None
        ),
        check_root=False,
    )
    wait_for(
        lambda: (
            runtime_info(host_state, destination_id)["runtime_lifecycle"] == "stopped"
        ),
        f"{prefix}-destination-parked",
    )
    wait_for(
        lambda: process_identity_is_gone(
            int(destination["provider_pid"]), str(destination["provider_birth"])
        ),
        f"{prefix}-destination-provider-stopped",
        timeout=10,
    )

    before = ready_runtime(host_state, source_id)
    if before is None:
        raise AcceptanceFailure(f"{prefix}-source-not-ready-before-loss")
    loss_evidence = capture_provider_evidence(host_state, before)
    if remember is not None:
        remember(host_state, source_id, loss_evidence)
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
    wait_for(
        lambda: process_identity_is_gone(
            int(before["provider_pid"]), str(before["provider_birth"])
        ),
        f"{prefix}-old-provider-stopped",
        timeout=10,
    )
    assert_provider_group_stopped(
        loss_evidence,
        host_state.parent,
        check_root=False,
    )
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
    version: str | None = None
    cleanup_targets: list[
        tuple[Path, str, dict[str, str], Path, ProviderEvidence | None]
    ] = []
    cleanup_diagnostics: list[str] = []

    def remember_cleanup_evidence(
        state_root: Path,
        workstream_id: str,
        evidence: ProviderEvidence | None,
    ) -> None:
        for index, target in enumerate(cleanup_targets):
            if target[0] == state_root and target[1] == workstream_id:
                cleanup_targets[index] = (
                    target[0],
                    target[1],
                    target[2],
                    target[3],
                    evidence,
                )
                return

    ordinary_before = tmux_snapshot()
    try:
        if not args.confirm_live_opencode:
            raise AcceptanceBlocked(reason)
        repository_root = Path(__file__).resolve().parents[1]
        binary = repository_root / "target" / "debug" / "wsnav"
        if not binary.is_file():
            raise AcceptanceBlocked("candidate-binary-missing")
        version = run(["opencode", "--version"]).stdout.strip()
        if (
            not version
            or len(version.encode("utf-8")) > 256
            or any(character in version for character in "\x00\r\n")
        ):
            raise AcceptanceBlocked("opencode-version-probe-malformed")
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
            cleanup_targets.append(
                (local_state, source_id, local_env, local_root, None)
            )
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
                    (
                        local_state,
                        output_id(result.stdout),
                        local_env,
                        local_root,
                        None,
                    )
                )
            return result

        def local_reconcile(workstream_id: str) -> None:
            wsnav(binary, local_state, "status", workstream_id, env=local_env)

        local_source, _local_destination = accept_host_path(
            binary=binary,
            provider_env=local_env,
            project=local_project,
            host_state=local_state,
            invoke=local_invoke,
            register=local_register,
            reconcile=local_reconcile,
            assertions=assertions,
            prefix="local",
            remember=remember_cleanup_evidence,
        )
        local_source_info = runtime_info(local_state, local_source)
        if local_source_info is None:
            raise AcceptanceFailure("local-source-cleanup-state-missing")
        park_with_observation(
            local_state,
            local_source_info,
            lambda: wsnav(binary, local_state, "park", local_source, env=local_env),
            local_root,
            on_capture=lambda evidence: remember_cleanup_evidence(
                local_state, local_source, evidence
            ),
        )
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
            cleanup_targets.append(
                (remote_state, source_id, remote_env, remote_root, None)
            )
            return registered

        def remote_fork(
            workstream_id: str, source: dict[str, Any]
        ) -> subprocess.CompletedProcess[str]:
            """Fork one exact settled boundary, with one safe refresh retry."""

            effect_baseline = fork_effect_baseline(remote_state, workstream_id)
            result = invoke_fork_with_revision_retry(
                workstream_id,
                source,
                invoke=lambda source_id, revision: wsnav(
                    binary,
                    client_state,
                    "host",
                    "fork",
                    "remote",
                    source_id,
                    revision,
                    env=client_env,
                    check=False,
                ),
                refresh=lambda baseline: wait_for_stable_settled_source(
                    remote_state,
                    workstream_id,
                    baseline=baseline,
                ),
                assert_no_effect=lambda: assert_fork_effect_unchanged(
                    remote_state,
                    workstream_id,
                    effect_baseline,
                ),
            )
            cleanup_targets.append(
                (
                    remote_state,
                    output_id(result.stdout),
                    remote_env,
                    remote_root,
                    None,
                )
            )
            return result

        def remote_invoke(arguments: list[str]) -> subprocess.CompletedProcess[str]:
            action, workstream_id, revision = arguments
            command = ["host", action, "remote", workstream_id, revision]
            result = wsnav(binary, client_state, *command, env=client_env, check=False)
            if result.returncode != 0:
                raise AcceptanceFailure(bounded_wsnav_failure(result, f"host-{action}"))
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
            remember=remember_cleanup_evidence,
            fork_invoke=remote_fork,
        )
        remote_source_info = runtime_info(remote_state, remote_source)
        remote_destination_info = runtime_info(remote_state, remote_destination)
        if remote_source_info is None or remote_destination_info is None:
            raise AcceptanceFailure("ssh-cleanup-state-missing")
        park_with_observation(
            remote_state,
            remote_source_info,
            lambda: remote_invoke(
                ["park", remote_source, str(remote_source_info["revision"])]
            ),
            remote_root,
            on_capture=lambda evidence: remember_cleanup_evidence(
                remote_state, remote_source, evidence
            ),
            check_root=False,
        )
        wait_for(
            lambda: process_identity_is_gone(
                int(remote_source_info["provider_pid"]),
                str(remote_source_info["provider_birth"]),
            ),
            "ssh-source-provider-stopped",
            timeout=10,
        )
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
        primary_status, primary_reason = status, reason
        cleanup_failed = False
        # Stop the disposable loopback daemon before any broad root scan. Its
        # command/config paths intentionally live under the same temp root.
        if sshd is not None and sshd.poll() is None:
            try:
                sshd.terminate()
                sshd.wait(timeout=10)
            except subprocess.TimeoutExpired:
                try:
                    sshd.kill()
                    sshd.wait(timeout=10)
                except (OSError, subprocess.TimeoutExpired):
                    cleanup_failed = True
                    record_cleanup_diagnostic(
                        cleanup_diagnostics, "cleanup-error:sshd-termination"
                    )
            except OSError:
                cleanup_failed = True
                record_cleanup_diagnostic(
                    cleanup_diagnostics, "cleanup-error:sshd-termination"
                )
        if root is not None:
            binary = Path(__file__).resolve().parents[1] / "target" / "debug" / "wsnav"
            for (
                state_root,
                workstream_id,
                environment,
                reference_root,
                fallback_evidence,
            ) in reversed(cleanup_targets):
                try:
                    retry_cleanup_action(
                        partial(
                            park_direct,
                            binary,
                            state_root,
                            workstream_id,
                            environment,
                            reference_root=reference_root,
                            fallback_evidence=fallback_evidence,
                            # A sibling target may still own the same disposable
                            # root. Defer that broad scan until every exact target
                            # has had its cleanup attempt.
                            check_root=False,
                        )
                    )
                except (
                    AcceptanceFailure,
                    OSError,
                    sqlite3.Error,
                    subprocess.TimeoutExpired,
                ) as error:
                    cleanup_failed = True
                    record_cleanup_diagnostic(
                        cleanup_diagnostics, bounded_cleanup_reason(error)
                    )
        if root is not None:
            # Any socket left here was not removed by a successful, ordered
            # Runtime cleanup. Never raw-kill it: an untracked provider may
            # still own the server and the disposable root must be preserved.
            try:
                sockets = list(root.rglob("tmux.sock"))
            except OSError as error:
                sockets = []
                cleanup_failed = True
                record_cleanup_diagnostic(
                    cleanup_diagnostics, bounded_cleanup_reason(error)
                )
            if sockets:
                cleanup_failed = True
                record_cleanup_diagnostic(
                    cleanup_diagnostics, "cleanup-private-runtime-artifacts-present"
                )
            # Root-wide process inspection is intentionally after every
            # registered target.  A bounded wait here only covers final
            # teardown/reaping; it cannot erase a strict post-Park failure
            # already recorded above.
            reference = wait_for_root_quiet(root)
            if reference is not None:
                cleanup_failed = True
                record_cleanup_diagnostic(
                    cleanup_diagnostics, format_root_reference(reference)
                )
            if not cleanup_failed:
                try:
                    shutil.rmtree(root)
                except OSError as error:
                    cleanup_failed = True
                    record_cleanup_diagnostic(
                        cleanup_diagnostics, bounded_cleanup_reason(error)
                    )
            if not cleanup_failed:
                reference = wait_for_root_removed(root)
                if reference is not None:
                    cleanup_failed = True
                    record_cleanup_diagnostic(
                        cleanup_diagnostics, format_root_reference(reference)
                    )
            assertions["cleanup_complete"] = not cleanup_failed and not root.exists()
            if cleanup_failed:
                status = "falsified"
                reason = compose_cleanup_reason(primary_reason, cleanup_diagnostics)
        assertions["ordinary_tmux_unchanged"] = tmux_snapshot() == ordinary_before

    result = {
        "study": "opencode-production-d8.2",
        "status": status,
        "reason": reason,
        "primary_status": primary_status,
        "primary_reason": primary_reason,
        "cleanup_diagnostics": cleanup_diagnostics,
        "versions": {"opencode": version},
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
