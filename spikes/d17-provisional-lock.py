#!/usr/bin/env python3
"""Falsify the D17 schema-14 stable provisional-lock lifecycle.

This disposable probe uses a private SQLite database and mode-0700 state root.
It starts no provider and never opens ordinary WSNav state.  It establishes the
filesystem and crash-window properties required before the real state migration
and onboarding actors can share ``provisional.lock``.

Only sanitized booleans, enums, and local tool versions are retained after the
temporary root has been removed.
"""

from __future__ import annotations

import argparse
import errno
import fcntl
import hashlib
import json
import os
import shutil
import sqlite3
import stat
import subprocess
import sys
import tempfile
import time
import uuid
from pathlib import Path
from typing import Any, Final

STUDY: Final = "d17-provisional-lock"
CONTRACT: Final = "schema14-provisional-lock-v1"
ROOT_PREFIX: Final = "wsnav-d17-provisional-lock."
LOCK_NAME: Final = "provisional.lock"
LOCK_FORMAT: Final = "wsnav-provisional-lock-v1"
LOCK_MODE: Final = 0o600
ROOT_MODE: Final = 0o700
MAX_LOCK_BYTES: Final = 512
COMMAND_TIMEOUT_SECONDS: Final = 4.0
POLL_SECONDS: Final = 0.02


class SpikeFailure(RuntimeError):
    """A probe result contradicts the proposed contract."""


class LockRejected(RuntimeError):
    """The lock evidence is unsafe, absent, or currently busy."""


def write_result(path: Path, value: dict[str, Any]) -> None:
    encoded = (json.dumps(value, sort_keys=True, indent=2) + "\n").encode("utf-8")
    descriptor = os.open(
        path,
        os.O_WRONLY | os.O_CREAT | os.O_TRUNC | os.O_CLOEXEC,
        LOCK_MODE,
    )
    try:
        os.fchmod(descriptor, LOCK_MODE)
        os.write(descriptor, encoded)
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def tool_version(command: str, argument: str) -> str:
    try:
        result = subprocess.run(
            [command, argument],
            capture_output=True,
            check=False,
            text=True,
            timeout=COMMAND_TIMEOUT_SECONDS,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        raise SpikeFailure(f"{command}-version-unavailable") from error
    if result.returncode != 0 or not result.stdout.strip():
        raise SpikeFailure(f"{command}-version-unavailable")
    first_line = result.stdout.splitlines()[0].strip()
    if len(first_line) > 160:
        raise SpikeFailure(f"{command}-version-malformed")
    return first_line


def fsync_directory(path: Path) -> None:
    flags = os.O_RDONLY | os.O_CLOEXEC
    if hasattr(os, "O_DIRECTORY"):
        flags |= os.O_DIRECTORY
    descriptor = os.open(path, flags)
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def canonical_root(root: Path) -> Path:
    try:
        resolved = root.resolve(strict=True)
        metadata = resolved.stat()
    except OSError as error:
        raise LockRejected("state-root-unavailable") from error
    if not stat.S_ISDIR(metadata.st_mode):
        raise LockRejected("state-root-unsafe")
    if metadata.st_uid != os.getuid() or stat.S_IMODE(metadata.st_mode) != ROOT_MODE:
        raise LockRejected("state-root-unsafe")
    return resolved


def lock_path(root: Path) -> Path:
    return canonical_root(root) / LOCK_NAME


def expected_content(host_id: str, lease_generation: int) -> bytes:
    if len(host_id) != 32 or any(
        character not in "0123456789abcdef" for character in host_id
    ):
        raise SpikeFailure("host-id-invalid")
    if lease_generation <= 0:
        raise SpikeFailure("lease-generation-invalid")
    encoded = (
        json.dumps(
            {
                "format": LOCK_FORMAT,
                "host_id": host_id,
                "lease_generation": lease_generation,
            },
            sort_keys=True,
            separators=(",", ":"),
        )
        + "\n"
    ).encode("ascii")
    if len(encoded) > MAX_LOCK_BYTES:
        raise SpikeFailure("lock-content-oversized")
    return encoded


def open_existing(path: Path) -> int:
    flags = os.O_RDWR | os.O_CLOEXEC
    if hasattr(os, "O_NOFOLLOW"):
        flags |= os.O_NOFOLLOW
    try:
        return os.open(path, flags)
    except OSError as error:
        if error.errno in (errno.ELOOP, errno.ENOENT, errno.ENOTDIR):
            raise LockRejected("lock-artifact-unsafe") from error
        raise LockRejected("lock-artifact-unavailable") from error


def validate_descriptor(descriptor: int, expected: bytes) -> os.stat_result:
    try:
        metadata = os.fstat(descriptor)
        if not stat.S_ISREG(metadata.st_mode):
            raise LockRejected("lock-artifact-unsafe")
        if (
            metadata.st_uid != os.getuid()
            or stat.S_IMODE(metadata.st_mode) != LOCK_MODE
        ):
            raise LockRejected("lock-artifact-unsafe")
        os.lseek(descriptor, 0, os.SEEK_SET)
        encoded = os.read(descriptor, MAX_LOCK_BYTES + 1)
    except OSError as error:
        raise LockRejected("lock-artifact-unavailable") from error
    if len(encoded) > MAX_LOCK_BYTES or encoded != expected:
        raise LockRejected("lock-artifact-mismatched")
    if fcntl.fcntl(descriptor, fcntl.F_GETFD) & fcntl.FD_CLOEXEC == 0:
        raise SpikeFailure("lock-fd-inheritable")
    return metadata


def validate_path_identity(path: Path, descriptor_metadata: os.stat_result) -> None:
    try:
        path_metadata = os.lstat(path)
    except OSError as error:
        raise LockRejected("lock-artifact-missing") from error
    if not stat.S_ISREG(path_metadata.st_mode):
        raise LockRejected("lock-artifact-unsafe")
    if (
        path_metadata.st_dev != descriptor_metadata.st_dev
        or path_metadata.st_ino != descriptor_metadata.st_ino
    ):
        raise LockRejected("lock-artifact-replaced")


def acquire_exclusive(descriptor: int) -> None:
    try:
        fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError as error:
        raise LockRejected("lock-busy") from error
    except OSError as error:
        raise LockRejected("lock-unavailable") from error


def release(descriptor: int) -> None:
    try:
        fcntl.flock(descriptor, fcntl.LOCK_UN)
    finally:
        os.close(descriptor)


def connect(database_path: Path) -> sqlite3.Connection:
    connection = sqlite3.connect(database_path, isolation_level=None)
    connection.execute("PRAGMA foreign_keys = ON")
    connection.execute("PRAGMA journal_mode = DELETE")
    connection.execute("PRAGMA synchronous = FULL")
    connection.execute("CREATE TABLE schema_meta (version INTEGER NOT NULL)")
    connection.execute("INSERT INTO schema_meta (version) VALUES (13)")
    connection.execute(
        "CREATE TABLE host_lease ("
        "host_id TEXT PRIMARY KEY NOT NULL, "
        "lease_generation INTEGER NOT NULL, "
        "phase TEXT NOT NULL, "
        "device INTEGER, "
        "inode INTEGER"
        ")"
    )
    return connection


def schema_version(connection: sqlite3.Connection) -> int:
    row = connection.execute("SELECT version FROM schema_meta").fetchone()
    if row is None or not isinstance(row[0], int):
        raise SpikeFailure("schema-version-missing")
    return row[0]


def read_lease(
    connection: sqlite3.Connection, host_id: str
) -> tuple[int, str, int | None, int | None]:
    row = connection.execute(
        "SELECT lease_generation, phase, device, inode FROM host_lease WHERE host_id = ?",
        (host_id,),
    ).fetchone()
    if row is None:
        raise LockRejected("lease-metadata-missing")
    generation, phase, device, inode = row
    if (
        not isinstance(generation, int)
        or phase not in ("pending", "ready")
        or (device is not None and not isinstance(device, int))
        or (inode is not None and not isinstance(inode, int))
    ):
        raise LockRejected("lease-metadata-malformed")
    return generation, phase, device, inode


def migrate_schema14(
    connection: sqlite3.Connection, root: Path, host_id: str, generation: int
) -> None:
    """Commit pending metadata only after rejecting a pre-schema artifact."""

    path = lock_path(root)
    try:
        os.lstat(path)
    except FileNotFoundError:
        pass
    except OSError as error:
        raise LockRejected("pre-schema-artifact-ambiguous") from error
    else:
        raise LockRejected("pre-schema-artifact-ambiguous")

    connection.execute("BEGIN IMMEDIATE")
    try:
        if schema_version(connection) != 13:
            raise LockRejected("schema-version-ambiguous")
        connection.execute("UPDATE schema_meta SET version = 14")
        connection.execute(
            "INSERT INTO host_lease (host_id, lease_generation, phase, device, inode) "
            "VALUES (?, ?, 'pending', NULL, NULL)",
            (host_id, generation),
        )
        connection.execute("COMMIT")
    except BaseException:
        connection.execute("ROLLBACK")
        raise


def create_lock(root: Path, expected: bytes) -> os.stat_result:
    path = lock_path(root)
    flags = os.O_RDWR | os.O_CREAT | os.O_EXCL | os.O_CLOEXEC
    if hasattr(os, "O_NOFOLLOW"):
        flags |= os.O_NOFOLLOW
    try:
        descriptor = os.open(path, flags, LOCK_MODE)
    except FileExistsError as error:
        raise LockRejected("lock-artifact-existing") from error
    except OSError as error:
        raise LockRejected("lock-create-failed") from error
    try:
        os.fchmod(descriptor, LOCK_MODE)
        view = memoryview(expected)
        while view:
            written = os.write(descriptor, view)
            if written <= 0:
                raise SpikeFailure("lock-write-failed")
            view = view[written:]
        os.fsync(descriptor)
        metadata = validate_descriptor(descriptor, expected)
    finally:
        os.close(descriptor)
    fsync_directory(root)
    return metadata


def install_pending(connection: sqlite3.Connection, root: Path, host_id: str) -> None:
    generation, phase, device, inode = read_lease(connection, host_id)
    if (
        schema_version(connection) != 14
        or phase != "pending"
        or device is not None
        or inode is not None
    ):
        raise LockRejected("lease-metadata-ambiguous")
    expected = expected_content(host_id, generation)
    path = lock_path(root)
    try:
        os.lstat(path)
    except FileNotFoundError:
        create_lock(root, expected)
    except OSError as error:
        raise LockRejected("lock-artifact-unavailable") from error

    descriptor = open_existing(path)
    try:
        metadata = validate_descriptor(descriptor, expected)
        acquire_exclusive(descriptor)
        validate_path_identity(path, metadata)
        connection.execute("BEGIN IMMEDIATE")
        try:
            latest_generation, latest_phase, latest_device, latest_inode = read_lease(
                connection, host_id
            )
            if (
                latest_generation != generation
                or latest_phase != "pending"
                or latest_device is not None
                or latest_inode is not None
            ):
                raise LockRejected("lease-metadata-raced")
            connection.execute(
                "UPDATE host_lease SET phase = 'ready', device = ?, inode = ? WHERE host_id = ?",
                (metadata.st_dev, metadata.st_ino, host_id),
            )
            connection.execute("COMMIT")
        except BaseException:
            connection.execute("ROLLBACK")
            raise
    finally:
        release(descriptor)


def acquire_ready(connection: sqlite3.Connection, root: Path, host_id: str) -> int:
    generation, phase, device, inode = read_lease(connection, host_id)
    if (
        schema_version(connection) != 14
        or phase != "ready"
        or device is None
        or inode is None
    ):
        raise LockRejected("lease-not-ready")
    expected = expected_content(host_id, generation)
    path = lock_path(root)
    descriptor = open_existing(path)
    try:
        metadata = validate_descriptor(descriptor, expected)
        if metadata.st_dev != device or metadata.st_ino != inode:
            raise LockRejected("lock-artifact-replaced")
        acquire_exclusive(descriptor)
        validate_path_identity(path, metadata)
        return descriptor
    except BaseException:
        os.close(descriptor)
        raise


def new_case(root: Path, name: str) -> tuple[Path, sqlite3.Connection, str]:
    state_root = root / name
    state_root.mkdir(mode=ROOT_MODE)
    state_root.chmod(ROOT_MODE)
    return state_root, connect(state_root / "state.sqlite"), uuid.uuid4().hex


def assert_rejected(action: Any, expected_reason: str) -> bool:
    try:
        action()
    except LockRejected as error:
        if str(error) != expected_reason:
            raise SpikeFailure("unexpected-refusal") from error
        return True
    raise SpikeFailure("unsafe-action-accepted")


def ordinary_tmux_fingerprint() -> str:
    environment = dict(os.environ)
    environment.pop("TMUX", None)
    try:
        result = subprocess.run(
            ["tmux", "list-sessions", "-F", "#{session_name}:#{session_created}"],
            capture_output=True,
            check=False,
            env=environment,
            text=True,
            timeout=COMMAND_TIMEOUT_SECONDS,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        raise SpikeFailure("ordinary-tmux-unavailable") from error
    if result.returncode != 0:
        return "absent"
    return hashlib.sha256(result.stdout.encode("utf-8")).hexdigest()


def wait_for_line(stream: Any, expected: str) -> None:
    deadline = time.monotonic() + COMMAND_TIMEOUT_SECONDS
    while time.monotonic() < deadline:
        line = stream.readline()
        if line == expected:
            return
        if line == "":
            break
    raise SpikeFailure("holder-not-ready")


def child_hold(root: Path, host_id: str) -> int:
    connection = sqlite3.connect(root / "state.sqlite", isolation_level=None)
    descriptor = acquire_ready(connection, root, host_id)
    try:
        print("locked", flush=True)
        time.sleep(COMMAND_TIMEOUT_SECONDS)
    finally:
        release(descriptor)
        connection.close()
    return 0


def child_cloexec(fd_value: str) -> int:
    try:
        descriptor = int(fd_value)
    except ValueError:
        return 64
    try:
        os.fstat(descriptor)
    except OSError as error:
        return 0 if error.errno == errno.EBADF else 65
    return 1


def run_probe() -> dict[str, Any]:
    before_tmux = ordinary_tmux_fingerprint()
    temporary_root = Path(tempfile.mkdtemp(prefix=ROOT_PREFIX))
    temporary_root.chmod(ROOT_MODE)
    assertions: dict[str, bool] = {}
    try:
        # A schema-13 owner must leave any unexpected stable artifact untouched.
        state_root, connection, host_id = new_case(temporary_root, "pre-schema")
        foreign = lock_path(state_root)
        foreign.write_text("foreign\n", encoding="ascii")
        foreign.chmod(LOCK_MODE)
        foreign_before = foreign.read_bytes()
        assertions["pre_schema_artifact_refused"] = assert_rejected(
            lambda: migrate_schema14(connection, state_root, host_id, 1),
            "pre-schema-artifact-ambiguous",
        )
        assertions["pre_schema_artifact_untouched"] = (
            schema_version(connection) == 13 and foreign.read_bytes() == foreign_before
        )
        connection.close()

        # Pending metadata is durable before create, then becomes ready only after
        # the exact current-owner artifact has been fsynced and identity-recorded.
        state_root, connection, host_id = new_case(temporary_root, "pending")
        migrate_schema14(connection, state_root, host_id, 7)
        generation, phase, device, inode = read_lease(connection, host_id)
        assertions["pending_precedes_create"] = (
            generation == 7
            and phase == "pending"
            and device is None
            and inode is None
            and not lock_path(state_root).exists()
        )
        install_pending(connection, state_root, host_id)
        generation, phase, device, inode = read_lease(connection, host_id)
        metadata = os.stat(lock_path(state_root), follow_symlinks=False)
        assertions["pending_finalizes_exact_ready_inode"] = (
            generation == 7
            and phase == "ready"
            and device == metadata.st_dev
            and inode == metadata.st_ino
            and metadata.st_uid == os.getuid()
            and stat.S_IMODE(metadata.st_mode) == LOCK_MODE
        )

        descriptor = acquire_ready(connection, state_root, host_id)
        try:
            duplicated = 200
            os.dup2(descriptor, duplicated, inheritable=False)
            try:
                inherited = subprocess.run(
                    [
                        sys.executable,
                        str(Path(__file__).resolve()),
                        "--internal-cloexec",
                        str(duplicated),
                    ],
                    check=False,
                    close_fds=False,
                    timeout=COMMAND_TIMEOUT_SECONDS,
                )
            finally:
                os.close(duplicated)
            assertions["ready_fd_is_cloexec"] = inherited.returncode == 0
        finally:
            release(descriptor)

        # A child that dies without unlinking releases only the kernel lease;
        # restart must re-acquire exactly the recorded artifact.
        holder = subprocess.Popen(
            [
                sys.executable,
                str(Path(__file__).resolve()),
                "--internal-hold",
                str(state_root),
                host_id,
            ],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
        if holder.stdout is None:
            raise SpikeFailure("holder-stdout-unavailable")
        wait_for_line(holder.stdout, "locked\n")
        assertions["busy_lock_refuses_without_second_artifact"] = (
            assert_rejected(
                lambda: acquire_ready(connection, state_root, host_id), "lock-busy"
            )
            and os.stat(lock_path(state_root), follow_symlinks=False).st_ino
            == metadata.st_ino
        )
        holder.kill()
        holder.wait(timeout=COMMAND_TIMEOUT_SECONDS)
        restarted = acquire_ready(connection, state_root, host_id)
        try:
            restarted_metadata = os.fstat(restarted)
            assertions["crash_restart_reuses_exact_inode"] = (
                restarted_metadata.st_dev == metadata.st_dev
                and restarted_metadata.st_ino == metadata.st_ino
            )
        finally:
            release(restarted)
        connection.close()

        # The post-create/pre-ready crash window may finalize only an exact file.
        state_root, connection, host_id = new_case(temporary_root, "crash-window")
        migrate_schema14(connection, state_root, host_id, 8)
        crash_metadata = create_lock(state_root, expected_content(host_id, 8))
        install_pending(connection, state_root, host_id)
        _, phase, device, inode = read_lease(connection, host_id)
        assertions["pending_crash_window_recovers_exact_artifact"] = (
            phase == "ready"
            and device == crash_metadata.st_dev
            and inode == crash_metadata.st_ino
        )
        connection.close()

        # Once ready, a missing, symlinked, replaced, or unlink-recreated path
        # is never normalized into a fresh usable stable lease.
        for case_name, mutation, reason in (
            ("ready-missing", "remove", "lock-artifact-unsafe"),
            ("ready-symlink", "symlink", "lock-artifact-unsafe"),
            ("ready-replaced", "replace", "lock-artifact-replaced"),
            ("ready-unlink-recreate", "recreate", "lock-artifact-replaced"),
        ):
            state_root, connection, host_id = new_case(temporary_root, case_name)
            migrate_schema14(connection, state_root, host_id, 9)
            install_pending(connection, state_root, host_id)
            path = lock_path(state_root)
            path_content = path.read_bytes()
            if mutation == "remove":
                path.unlink()
            elif mutation == "symlink":
                path.unlink()
                path.symlink_to("elsewhere")
            elif mutation == "replace":
                replacement = path.with_name("replacement")
                replacement.write_bytes(path_content)
                replacement.chmod(LOCK_MODE)
                os.replace(replacement, path)
            elif mutation == "recreate":
                path.unlink()
                path.write_bytes(path_content)
                path.chmod(LOCK_MODE)
            else:
                raise SpikeFailure("unknown-ready-mutation")
            assertions[f"{case_name}_refused"] = assert_rejected(
                lambda connection=connection, state_root=state_root, host_id=host_id: (
                    acquire_ready(connection, state_root, host_id)
                ),
                reason,
            )
            _, phase, _, _ = read_lease(connection, host_id)
            assertions[f"{case_name}_metadata_stays_ready"] = phase == "ready"
            connection.close()

        # Pending accepts neither a symlink nor a mismatched crash-window file.
        state_root, connection, host_id = new_case(temporary_root, "pending-unsafe")
        migrate_schema14(connection, state_root, host_id, 10)
        path = lock_path(state_root)
        path.symlink_to("elsewhere")
        assertions["pending_symlink_refused"] = assert_rejected(
            lambda: install_pending(connection, state_root, host_id),
            "lock-artifact-unsafe",
        )
        connection.close()

        state_root, connection, host_id = new_case(temporary_root, "pending-mismatch")
        migrate_schema14(connection, state_root, host_id, 11)
        path = lock_path(state_root)
        path.write_bytes(b"wrong\n")
        path.chmod(LOCK_MODE)
        assertions["pending_mismatch_refused"] = assert_rejected(
            lambda: install_pending(connection, state_root, host_id),
            "lock-artifact-mismatched",
        )
        connection.close()
    finally:
        shutil.rmtree(temporary_root, ignore_errors=True)

    after_tmux = ordinary_tmux_fingerprint()
    assertions["temporary_root_removed"] = not temporary_root.exists()
    assertions["ordinary_tmux_unchanged"] = before_tmux == after_tmux
    assertions["all_case_assertions_pass"] = all(assertions.values())
    return {
        "contract": CONTRACT,
        "status": "pass" if assertions["all_case_assertions_pass"] else "falsified",
        "reason": "schema14-stable-provisional-lock-observed",
        "tools": {
            "python": tool_version("python3", "--version"),
            "tmux": tool_version("tmux", "-V"),
        },
        "assertions": assertions,
    }


def parse_arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--result", type=Path)
    parser.add_argument("--internal-hold", nargs=2, metavar=("ROOT", "HOST_ID"))
    parser.add_argument("--internal-cloexec")
    return parser.parse_args()


def main() -> int:
    arguments = parse_arguments()
    if arguments.internal_cloexec is not None:
        return child_cloexec(arguments.internal_cloexec)
    if arguments.internal_hold is not None:
        root_text, host_id = arguments.internal_hold
        return child_hold(Path(root_text), host_id)
    if arguments.result is None:
        raise SpikeFailure("result-path-required")
    result = run_probe()
    arguments.result.parent.mkdir(parents=True, exist_ok=True)
    write_result(arguments.result, result)
    return 0 if result["status"] == "pass" else 1


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (LockRejected, SpikeFailure) as error:
        print(f"{STUDY}: {error}", file=sys.stderr)
        raise SystemExit(1) from error
