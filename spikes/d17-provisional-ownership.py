#!/usr/bin/env python3
"""Falsify the D17 marker-backed provisional ownership lifecycle.

The probe is a disposable model of the boundary between an unregistered shell
candidate and a durable Runtime.  A single host-private flock serializes marker
materialization, prepare, helper promotion, and pre-handoff cleanup.  It starts
no provider, uses only fake private runtime artifacts, and deletes all roots
before emitting a sanitized fixture.
"""

from __future__ import annotations

import argparse
import fcntl
import hashlib
import json
import os
import shutil
import stat
import subprocess
import tempfile
import uuid
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Final, Self

STUDY: Final = "d17-provisional-ownership"
CONTRACT: Final = "marker-lease-promotion-v1"
ROOT_PREFIX: Final = "wsnav-d17-provisional-ownership."
ROOT_MODE: Final = 0o700
FILE_MODE: Final = 0o600
MAX_MARKER_BYTES: Final = 8 * 1024
LOCK_NAME: Final = "provisional.lock"
MARKER_NAME: Final = "provisional.json"
COMMAND_TIMEOUT_SECONDS: Final = 4.0


class SpikeFailure(RuntimeError):
    """A lifecycle result contradicted the proposed D17 contract."""


class LifecycleRefused(RuntimeError):
    """Evidence is ambiguous, foreign, stale, or fenced."""


@dataclass(frozen=True)
class Candidate:
    presentation_id: str
    candidate_id: str
    slot_generation: str
    seed: Path

    def runtime_directory(self, root: Path) -> Path:
        return root / "run" / f"runtime-{self.candidate_id}"

    def socket(self, root: Path) -> Path:
        return self.runtime_directory(root) / "tmux.sock"

    def config(self, root: Path) -> Path:
        return self.runtime_directory(root) / "tmux.conf"

    def session_name(self) -> str:
        return f"wsnav-{self.candidate_id}"

    def marker_path(self, root: Path) -> Path:
        return root / "presentations" / self.presentation_id / MARKER_NAME


def private_directory(path: Path) -> None:
    path.mkdir(mode=ROOT_MODE, parents=True, exist_ok=False)
    path.chmod(ROOT_MODE)


def write_private(path: Path, value: bytes) -> None:
    if len(value) > MAX_MARKER_BYTES:
        raise SpikeFailure("private-record-oversized")
    descriptor = os.open(
        path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_CLOEXEC, FILE_MODE
    )
    try:
        os.fchmod(descriptor, FILE_MODE)
        os.write(descriptor, value)
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def read_private(path: Path) -> bytes:
    flags = os.O_RDONLY | os.O_CLOEXEC
    if hasattr(os, "O_NOFOLLOW"):
        flags |= os.O_NOFOLLOW
    try:
        descriptor = os.open(path, flags)
    except OSError as error:
        raise LifecycleRefused("private-record-unavailable") from error
    try:
        metadata = os.fstat(descriptor)
        if (
            not stat.S_ISREG(metadata.st_mode)
            or metadata.st_uid != os.getuid()
            or stat.S_IMODE(metadata.st_mode) != FILE_MODE
        ):
            raise LifecycleRefused("private-record-unsafe")
        value = os.read(descriptor, MAX_MARKER_BYTES + 1)
    finally:
        os.close(descriptor)
    if len(value) > MAX_MARKER_BYTES:
        raise LifecycleRefused("private-record-oversized")
    return value


def canonical_root(root: Path) -> Path:
    try:
        resolved = root.resolve(strict=True)
        metadata = resolved.stat()
    except OSError as error:
        raise LifecycleRefused("state-root-unavailable") from error
    if (
        not stat.S_ISDIR(metadata.st_mode)
        or metadata.st_uid != os.getuid()
        or stat.S_IMODE(metadata.st_mode) != ROOT_MODE
    ):
        raise LifecycleRefused("state-root-unsafe")
    return resolved


class Lease:
    def __init__(self, root: Path) -> None:
        self.root = canonical_root(root)
        self.descriptor: int | None = None

    def __enter__(self) -> Self:
        path = self.root / LOCK_NAME
        flags = os.O_RDWR | os.O_CLOEXEC
        if hasattr(os, "O_NOFOLLOW"):
            flags |= os.O_NOFOLLOW
        try:
            self.descriptor = os.open(path, flags)
            metadata = os.fstat(self.descriptor)
            if (
                not stat.S_ISREG(metadata.st_mode)
                or metadata.st_uid != os.getuid()
                or stat.S_IMODE(metadata.st_mode) != FILE_MODE
            ):
                raise LifecycleRefused("lease-unsafe")
            fcntl.flock(self.descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError as error:
            self.close()
            raise LifecycleRefused("lease-busy") from error
        except OSError as error:
            self.close()
            raise LifecycleRefused("lease-unavailable") from error
        return self

    def close(self) -> None:
        if self.descriptor is not None:
            try:
                fcntl.flock(self.descriptor, fcntl.LOCK_UN)
            finally:
                os.close(self.descriptor)
                self.descriptor = None

    def __exit__(self, *_: object) -> None:
        self.close()


def create_state(root: Path) -> None:
    private_directory(root)
    private_directory(root / "run")
    private_directory(root / "presentations")
    private_directory(root / "journal")
    private_directory(root / "registry")
    write_private(root / LOCK_NAME, b"schema14-stable-provisional-lock\n")


def marker_payload(root: Path, candidate: Candidate) -> bytes:
    payload = {
        "candidate_id": candidate.candidate_id,
        "config": str(candidate.config(root)),
        "directory": str(candidate.runtime_directory(root)),
        "presentation_id": candidate.presentation_id,
        "seed": str(candidate.seed),
        "session": candidate.session_name(),
        "slot_generation": candidate.slot_generation,
        "socket": str(candidate.socket(root)),
        "version": 1,
    }
    return (json.dumps(payload, sort_keys=True, separators=(",", ":")) + "\n").encode(
        "utf-8"
    )


def parse_marker(root: Path, path: Path) -> Candidate:
    try:
        payload = json.loads(read_private(path))
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise LifecycleRefused("marker-malformed") from error
    if not isinstance(payload, dict) or set(payload) != {
        "candidate_id",
        "config",
        "directory",
        "presentation_id",
        "seed",
        "session",
        "slot_generation",
        "socket",
        "version",
    }:
        raise LifecycleRefused("marker-malformed")
    if payload.get("version") != 1:
        raise LifecycleRefused("marker-malformed")
    fields = (
        "candidate_id",
        "presentation_id",
        "slot_generation",
        "seed",
        "directory",
        "socket",
        "config",
        "session",
    )
    if any(not isinstance(payload[field], str) for field in fields):
        raise LifecycleRefused("marker-malformed")
    try:
        candidate = Candidate(
            presentation_id=payload["presentation_id"],
            candidate_id=payload["candidate_id"],
            slot_generation=payload["slot_generation"],
            seed=Path(payload["seed"]).resolve(strict=True),
        )
        uuid.UUID(hex=candidate.presentation_id)
        uuid.UUID(hex=candidate.candidate_id)
        uuid.UUID(hex=candidate.slot_generation)
    except (OSError, ValueError, AttributeError) as error:
        raise LifecycleRefused("marker-malformed") from error
    if (
        payload["directory"] != str(candidate.runtime_directory(root))
        or payload["socket"] != str(candidate.socket(root))
        or payload["config"] != str(candidate.config(root))
        or payload["session"] != candidate.session_name()
    ):
        raise LifecycleRefused("marker-mismatched")
    return candidate


def candidate_artifacts_exact(root: Path, candidate: Candidate) -> bool:
    directory = candidate.runtime_directory(root)
    try:
        metadata = directory.lstat()
    except OSError:
        return False
    if (
        not stat.S_ISDIR(metadata.st_mode)
        or directory.is_symlink()
        or stat.S_IMODE(metadata.st_mode) != ROOT_MODE
    ):
        return False
    allowed = {"tmux.conf", "tmux.sock"}
    try:
        entries = {entry.name for entry in directory.iterdir()}
    except OSError:
        return False
    if entries != allowed:
        return False
    return all(
        path.is_file()
        and not path.is_symlink()
        and path.stat().st_uid == os.getuid()
        and stat.S_IMODE(path.stat().st_mode) == FILE_MODE
        for path in (candidate.socket(root), candidate.config(root))
    )


def marker_candidates(root: Path) -> list[Candidate]:
    presentation_root = root / "presentations"
    try:
        entries = sorted(presentation_root.iterdir(), key=lambda entry: entry.name)
    except OSError as error:
        raise LifecycleRefused("presentation-namespace-unavailable") from error
    candidates: list[Candidate] = []
    for entry in entries:
        if not entry.is_dir() or entry.is_symlink():
            raise LifecycleRefused("presentation-namespace-ambiguous")
        marker = entry / MARKER_NAME
        if marker.exists() or marker.is_symlink():
            candidate = parse_marker(root, marker)
            if candidate.presentation_id != entry.name:
                raise LifecycleRefused("marker-mismatched")
            candidates.append(candidate)
        elif any(entry.iterdir()):
            raise LifecycleRefused("presentation-namespace-ambiguous")
    return candidates


def owned_runtime_ids(root: Path) -> set[str]:
    path = root / "registry" / "owned.json"
    if not path.exists():
        return set()
    try:
        value = json.loads(read_private(path))
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise LifecycleRefused("registry-malformed") from error
    if not isinstance(value, dict) or set(value) != {"owned_runtime_ids"}:
        raise LifecycleRefused("registry-malformed")
    identifiers = value["owned_runtime_ids"]
    if not isinstance(identifiers, list) or len(identifiers) > 32:
        raise LifecycleRefused("registry-malformed")
    result: set[str] = set()
    for identifier in identifiers:
        if not isinstance(identifier, str):
            raise LifecycleRefused("registry-malformed")
        try:
            uuid.UUID(hex=identifier)
        except ValueError as error:
            raise LifecycleRefused("registry-malformed") from error
        result.add(identifier)
    if len(result) != len(identifiers):
        raise LifecycleRefused("registry-malformed")
    return result


def write_owned(root: Path, identifiers: set[str]) -> None:
    path = root / "registry" / "owned.json"
    if path.exists():
        path.unlink()
    payload = {"owned_runtime_ids": sorted(identifiers)}
    write_private(
        path, (json.dumps(payload, separators=(",", ":")) + "\n").encode("ascii")
    )


def classify(root: Path, presentation_id: str) -> str:
    markers = marker_candidates(root)
    if len(markers) > 1:
        return "ambiguous"
    owned = owned_runtime_ids(root)
    directories = sorted((root / "run").iterdir(), key=lambda entry: entry.name)
    marker = markers[0] if markers else None
    expected_candidate = marker.candidate_id if marker else None
    for directory in directories:
        if not directory.name.startswith("runtime-"):
            return "ambiguous"
        identifier = directory.name.removeprefix("runtime-")
        try:
            uuid.UUID(hex=identifier)
        except ValueError:
            return "ambiguous"
        if identifier in owned:
            continue
        if identifier != expected_candidate:
            return "ambiguous"
    if marker is None:
        return "clean"
    if not candidate_artifacts_exact(root, marker):
        return "ambiguous"
    return "same" if marker.presentation_id == presentation_id else "busy"


def create_candidate_artifacts(root: Path, candidate: Candidate) -> None:
    directory = candidate.runtime_directory(root)
    try:
        private_directory(directory)
        write_private(candidate.socket(root), b"synthetic-tmux-socket\n")
        write_private(candidate.config(root), b"synthetic-tmux-config\n")
    except FileExistsError as error:
        raise LifecycleRefused("candidate-collision") from error


def materialize(
    root: Path, presentation_id: str, seed: Path, candidate_id: str | None = None
) -> Candidate:
    with Lease(root):
        state = classify(root, presentation_id)
        if state == "same":
            marker = marker_candidates(root)[0]
            return marker
        if state == "busy":
            raise LifecycleRefused("candidate-owned-by-other-presentation")
        if state != "clean":
            raise LifecycleRefused("candidate-namespace-ambiguous")
        try:
            resolved_seed = seed.resolve(strict=True)
        except OSError as error:
            raise LifecycleRefused("seed-unavailable") from error
        if not resolved_seed.is_dir():
            raise LifecycleRefused("seed-unsafe")
        candidate = Candidate(
            presentation_id=presentation_id,
            candidate_id=candidate_id or uuid.uuid4().hex,
            slot_generation=uuid.uuid4().hex,
            seed=resolved_seed,
        )
        try:
            uuid.UUID(hex=presentation_id)
            uuid.UUID(hex=candidate.candidate_id)
        except ValueError as error:
            raise LifecycleRefused("candidate-identity-malformed") from error
        presentation_directory = root / "presentations" / presentation_id
        private_directory(presentation_directory)
        create_candidate_artifacts(root, candidate)
        write_private(candidate.marker_path(root), marker_payload(root, candidate))
        return candidate


def journal_path(root: Path, candidate: Candidate) -> Path:
    return root / "journal" / f"{candidate.slot_generation}.json"


def candidate_digest(root: Path, candidate: Candidate) -> str:
    return hashlib.sha256(marker_payload(root, candidate)).hexdigest()


def write_journal(root: Path, candidate: Candidate, phase: str) -> None:
    path = journal_path(root, candidate)
    if path.exists():
        path.unlink()
    value = {
        "candidate_digest": candidate_digest(root, candidate),
        "candidate_id": candidate.candidate_id,
        "phase": phase,
        "presentation_id": candidate.presentation_id,
        "slot_generation": candidate.slot_generation,
    }
    write_private(
        path,
        (json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n").encode(
            "ascii"
        ),
    )


def read_journal(root: Path, candidate: Candidate) -> str:
    try:
        value = json.loads(read_private(journal_path(root, candidate)))
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise LifecycleRefused("journal-malformed") from error
    expected = {
        "candidate_digest": candidate_digest(root, candidate),
        "candidate_id": candidate.candidate_id,
        "presentation_id": candidate.presentation_id,
        "slot_generation": candidate.slot_generation,
    }
    if not isinstance(value, dict) or any(
        value.get(key) != item for key, item in expected.items()
    ):
        raise LifecycleRefused("journal-mismatched")
    phase = value.get("phase")
    if phase not in ("issued", "runtime_owned_launching", "exec_proven", "cancelled"):
        raise LifecycleRefused("journal-malformed")
    return phase


def prepare(root: Path, candidate: Candidate) -> None:
    with Lease(root):
        parsed = parse_marker(root, candidate.marker_path(root))
        if parsed != candidate or classify(root, candidate.presentation_id) != "same":
            raise LifecycleRefused("prepare-marker-mismatch")
        if journal_path(root, candidate).exists():
            raise LifecycleRefused("prepare-already-issued")
        write_journal(root, candidate, "issued")


def remove_candidate_artifacts(root: Path, candidate: Candidate) -> None:
    if not candidate_artifacts_exact(root, candidate):
        raise LifecycleRefused("candidate-cleanup-ambiguous")
    for path in (candidate.socket(root), candidate.config(root)):
        path.unlink()
    candidate.runtime_directory(root).rmdir()


def cleanup(root: Path, candidate: Candidate) -> str:
    with Lease(root):
        try:
            parsed = parse_marker(root, candidate.marker_path(root))
        except LifecycleRefused:
            if candidate.candidate_id in owned_runtime_ids(root):
                return "post-ownership-noop"
            raise
        if parsed != candidate:
            raise LifecycleRefused("cleanup-marker-mismatch")
        phase = (
            read_journal(root, candidate)
            if journal_path(root, candidate).exists()
            else "unprepared"
        )
        if phase in ("runtime_owned_launching", "exec_proven"):
            raise LifecycleRefused("cleanup-after-ownership")
        if phase == "issued":
            write_journal(root, candidate, "cancelled")
        remove_candidate_artifacts(root, candidate)
        candidate.marker_path(root).unlink()
        candidate.marker_path(root).parent.rmdir()
        return "pre-handoff-cleaned"


def promote(root: Path, candidate: Candidate) -> None:
    with Lease(root):
        parsed = parse_marker(root, candidate.marker_path(root))
        if parsed != candidate or classify(root, candidate.presentation_id) != "same":
            raise LifecycleRefused("promote-marker-mismatch")
        if read_journal(root, candidate) != "issued":
            raise LifecycleRefused("capability-unavailable")
        owned = owned_runtime_ids(root)
        if candidate.candidate_id in owned:
            raise LifecycleRefused("runtime-already-owned")
        # The durable ownership record is intentionally first: a later marker
        # deletion cannot restore presentation cleanup authority.
        owned.add(candidate.candidate_id)
        write_owned(root, owned)
        write_journal(root, candidate, "runtime_owned_launching")
        candidate.marker_path(root).unlink()
        candidate.marker_path(root).parent.rmdir()


def action_allowed(root: Path, candidate: Candidate) -> bool:
    if candidate.candidate_id not in owned_runtime_ids(root):
        return False
    return read_journal(root, candidate) == "exec_proven"


def prove_exec(root: Path, candidate: Candidate) -> None:
    with Lease(root):
        if candidate.candidate_id not in owned_runtime_ids(root):
            raise LifecycleRefused("runtime-not-owned")
        if read_journal(root, candidate) != "runtime_owned_launching":
            raise LifecycleRefused("exec-phase-invalid")
        write_journal(root, candidate, "exec_proven")


def run_git(arguments: list[str], cwd: Path) -> str:
    result = subprocess.run(
        ["git", "-C", str(cwd), *arguments],
        capture_output=True,
        check=False,
        text=True,
        timeout=COMMAND_TIMEOUT_SECONDS,
    )
    if result.returncode != 0:
        raise SpikeFailure("git-command-failed")
    return result.stdout.strip()


def git_root(cwd: Path) -> Path:
    try:
        top = run_git(["rev-parse", "--show-toplevel"], cwd)
        bare = run_git(["rev-parse", "--is-bare-repository"], cwd)
    except SpikeFailure as error:
        raise LifecycleRefused("non-git-root") from error
    if bare != "false":
        raise LifecycleRefused("bare-git-root")
    try:
        root = Path(top).resolve(strict=True)
        cwd.resolve(strict=True).relative_to(root)
    except (OSError, ValueError) as error:
        raise LifecycleRefused("git-root-ambiguous") from error
    return root


def assert_refused(action: Any, reason: str) -> bool:
    try:
        action()
    except LifecycleRefused as error:
        if str(error) != reason:
            raise SpikeFailure("unexpected-refusal") from error
        return True
    raise SpikeFailure("unsafe-action-accepted")


def run_probe() -> dict[str, object]:
    temporary = Path(tempfile.mkdtemp(prefix=ROOT_PREFIX))
    temporary.chmod(ROOT_MODE)
    assertions: dict[str, bool] = {}
    try:
        repository = temporary / "repository"
        repository.mkdir(mode=ROOT_MODE)
        run_git(["init"], repository)
        run_git(["config", "user.email", "spike@example.invalid"], repository)
        run_git(["config", "user.name", "WSNav Spike"], repository)
        (repository / "tracked").write_text("x\n", encoding="ascii")
        run_git(["add", "tracked"], repository)
        run_git(["commit", "-m", "initial"], repository)
        linked = temporary / "linked"
        run_git(["worktree", "add", str(linked)], repository)
        linked_child = linked / "child"
        linked_child.mkdir()
        assertions["git_root_keeps_linked_worktree_exact"] = (
            git_root(linked_child) == linked.resolve()
        )
        assertions["non_git_seed_refuses"] = assert_refused(
            lambda: git_root(temporary), "non-git-root"
        )

        root = temporary / "state"
        create_state(root)
        first_presentation = uuid.uuid4().hex
        second_presentation = uuid.uuid4().hex
        candidate = materialize(root, first_presentation, linked_child)
        assertions["materialization_uses_full_uuid_final_paths"] = (
            len(candidate.candidate_id) == 32
            and candidate.runtime_directory(root).name
            == f"runtime-{candidate.candidate_id}"
            and candidate.socket(root)
            == candidate.runtime_directory(root) / "tmux.sock"
            and candidate.config(root)
            == candidate.runtime_directory(root) / "tmux.conf"
            and candidate.session_name() == f"wsnav-{candidate.candidate_id}"
        )
        assertions["marker_only_candidate_is_excluded_from_registry"] = (
            owned_runtime_ids(root) == set()
            and classify(root, first_presentation) == "same"
        )
        assertions["second_presentation_is_busy_without_second_candidate"] = (
            assert_refused(
                lambda: materialize(root, second_presentation, linked_child),
                "candidate-owned-by-other-presentation",
            )
            and len(list((root / "run").iterdir())) == 1
        )

        prepare(root, candidate)
        assertions["unproven_runtime_is_action_fenced"] = not action_allowed(
            root, candidate
        )
        assertions["cleanup_wins_before_helper_consume"] = (
            cleanup(root, candidate) == "pre-handoff-cleaned"
        )
        assertions["cleanup_cancels_without_owned_runtime_or_residue"] = (
            owned_runtime_ids(root) == set()
            and list((root / "run").iterdir()) == []
            and classify(root, first_presentation) == "clean"
        )
        assertions["cancelled_helper_cannot_promote"] = assert_refused(
            lambda: promote(root, candidate), "private-record-unavailable"
        )

        promoted = materialize(root, first_presentation, linked_child)
        prepare(root, promoted)
        promote(root, promoted)
        assertions["ownership_commit_precedes_marker_cleanup"] = (
            promoted.candidate_id in owned_runtime_ids(root)
            and not promoted.marker_path(root).exists()
            and read_journal(root, promoted) == "runtime_owned_launching"
        )
        assertions["post_ownership_cleanup_never_signals_or_removes_runtime"] = (
            cleanup(root, promoted) == "post-ownership-noop"
            and promoted.runtime_directory(root).exists()
        )
        assertions["duplicate_helper_refuses_after_consume"] = assert_refused(
            lambda: promote(root, promoted), "private-record-unavailable"
        )
        assertions["runtime_stays_fenced_until_exec_proof"] = not action_allowed(
            root, promoted
        )
        prove_exec(root, promoted)
        assertions["exec_proof_releases_action_fence"] = action_allowed(root, promoted)

        fresh = materialize(root, second_presentation, linked_child)
        assertions["promotion_derives_one_fresh_provisional_candidate"] = (
            fresh.candidate_id != promoted.candidate_id
            and classify(root, second_presentation) == "same"
            and len(list((root / "run").iterdir())) == 2
        )

        collision_root = temporary / "collision-state"
        create_state(collision_root)
        collision_id = uuid.uuid4().hex
        collision_directory = collision_root / "run" / f"runtime-{collision_id}"
        private_directory(collision_directory)
        assertions["foreign_candidate_collision_refuses_without_adoption"] = (
            assert_refused(
                lambda: materialize(
                    collision_root, uuid.uuid4().hex, linked_child, collision_id
                ),
                "candidate-namespace-ambiguous",
            )
            and collision_directory.exists()
        )

        missing_marker_root = temporary / "missing-marker-state"
        create_state(missing_marker_root)
        missing = materialize(missing_marker_root, uuid.uuid4().hex, linked_child)
        missing.marker_path(missing_marker_root).unlink()
        assertions["markerless_runtime_shape_blocks_fresh_materialization"] = (
            assert_refused(
                lambda: materialize(
                    missing_marker_root, uuid.uuid4().hex, linked_child
                ),
                "candidate-namespace-ambiguous",
            )
            and missing.runtime_directory(missing_marker_root).exists()
        )
    finally:
        shutil.rmtree(temporary, ignore_errors=True)

    assertions["temporary_root_removed"] = not temporary.exists()
    assertions["all_case_assertions_pass"] = all(assertions.values())
    return {
        "contract": CONTRACT,
        "status": "pass" if assertions["all_case_assertions_pass"] else "falsified",
        "reason": "marker-backed-provisional-ownership-observed",
        "assertions": assertions,
    }


def parse_arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--result", type=Path, required=True)
    return parser.parse_args()


def main() -> int:
    arguments = parse_arguments()
    result = run_probe()
    encoded = (json.dumps(result, sort_keys=True, indent=2) + "\n").encode("utf-8")
    arguments.result.parent.mkdir(parents=True, exist_ok=True)
    arguments.result.write_bytes(encoded)
    arguments.result.chmod(FILE_MODE)
    return 0 if result["status"] == "pass" else 1


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (LifecycleRefused, SpikeFailure) as error:
        print(f"{STUDY}: {error}", file=os.sys.stderr)
        raise SystemExit(1) from error
