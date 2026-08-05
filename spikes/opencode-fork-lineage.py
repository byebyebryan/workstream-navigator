#!/usr/bin/env python3
"""Probe fork lineage recoverability via the opencode HTTP server API.

Creates a disposable source session, forks it, and checks whether the
destination is discoverable from the source by structural lineage
(GET /session/:id/children) or only by display text. Uses its own temporary
project and sessions; never touches other opencode sessions or tmux.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import socket
import sqlite3
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
from pathlib import Path
from typing import Any

STUDY = "opencode-fork-lineage"
MODEL = "opencode-go/deepseek-v4-flash"
MARKER = "WSNAV_OC_LINEAGE"
PROMPT = (
    f"Reply with the exact token {MARKER} and nothing else. "
    "Do not use tools, inspect files, or make changes."
)


class StudyFailure(RuntimeError):
    pass


class StudyBlocked(RuntimeError):
    pass


def free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def opencode_db() -> str:
    out = subprocess.run(
        ["opencode", "db", "path"], capture_output=True, text=True, check=True
    ).stdout.strip()
    return out


def request(url: str) -> Any:
    with urllib.request.urlopen(url, timeout=10) as resp:
        return json.loads(resp.read().decode())


def run_opencode(args: list[str], cwd: Path, timeout: int = 180) -> str:
    proc = subprocess.run(
        ["opencode", "run", "--model", MODEL, "--format", "json", *args],
        capture_output=True,
        text=True,
        timeout=timeout,
        cwd=cwd,
        check=False,
    )
    if proc.returncode != 0:
        raise StudyBlocked(f"opencode run failed: {proc.stderr[-400:]}")
    return proc.stdout


def first_session_id(out: str) -> str | None:
    for line in out.splitlines():
        try:
            ev = json.loads(line)
        except json.JSONDecodeError:
            continue
        if ev.get("type") == "step_start" and ev.get("sessionID"):
            return ev["sessionID"]
    return None


def digest(session_id: str) -> str:
    return hashlib.sha256(session_id.encode()).hexdigest()[:16]


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--result", type=Path)
    args = parser.parse_args()

    assertions = {
        "fork_via_cli_creates_distinct_session": False,
        "fork_discoverable_via_children_api": False,
        "fork_parent_id_structural": False,
        "fork_lineage_recoverable_from_source": False,
    }
    root: Path | None = None
    server: subprocess.Popen[str] | None = None
    source_id: str | None = None
    fork_id: str | None = None
    status = "blocked"
    reason = "harness-early-exit"
    cleanup_complete = True
    port = free_port()
    try:
        root = Path(tempfile.mkdtemp(prefix="wsnav-oclineage."))
        project = root / "project"
        project.mkdir()

        source_out = run_opencode([PROMPT], project)
        source_id = first_session_id(source_out)
        if source_id is None:
            raise StudyFailure("no source session id observed")
        time.sleep(1)

        # Fork via CLI (fork-and-continue).
        fork_out = run_opencode(
            ["--session", source_id, "--fork", PROMPT], project
        )
        candidate = first_session_id(fork_out)
        if candidate is None:
            raise StudyFailure("no fork session id observed")
        fork_id = candidate
        if fork_id == source_id:
            raise StudyFailure("fork returned the source session")
        assertions["fork_via_cli_creates_distinct_session"] = True

        # Structural parent linkage in the SQLite session table.
        conn = sqlite3.connect(opencode_db())
        row = conn.execute(
            "select parent_id from session where id = ?", (fork_id,)
        ).fetchone()
        conn.close()
        if row is not None and row[0] is not None:
            assertions["fork_parent_id_structural"] = True

        # Start a headless server bound to the same DB and query children.
        server = subprocess.Popen(
            ["opencode", "serve", "--port", str(port), "--hostname", "127.0.0.1"],
            cwd=project,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            text=True,
        )
        base = f"http://127.0.0.1:{port}"
        deadline = time.time() + 30
        children = None
        while time.time() < deadline:
            try:
                children = request(f"{base}/session/{source_id}/children")
                break
            except (urllib.error.URLError, json.JSONDecodeError):
                time.sleep(1)
        if children is None:
            raise StudyBlocked("opencode server did not start in time")

        if any(isinstance(c, dict) and c.get("id") == fork_id for c in children):
            assertions["fork_discoverable_via_children_api"] = True
        if children and fork_id is not None:
            ids = [
                c.get("id") if isinstance(c, dict) else c
                for c in children
            ]
            if fork_id in ids:
                assertions["fork_lineage_recoverable_from_source"] = True

        status = "pass"
        reason = "fork-lineage-recoverability-recorded"
    except StudyBlocked as error:
        status, reason = "blocked", str(error)
    except StudyFailure as error:
        status, reason = "falsified", str(error)
    except (subprocess.TimeoutExpired, OSError, sqlite3.Error) as error:
        status, reason = "blocked", f"harness-error:{type(error).__name__}"
    finally:
        if server is not None and server.poll() is None:
            server.terminate()
            try:
                server.wait(timeout=10)
            except subprocess.TimeoutExpired:
                server.kill()
                server.wait(timeout=10)
        if root is not None:
            import shutil

            shutil.rmtree(root, ignore_errors=True)
            cleanup_complete = not root.exists()

    result = {
        "study": STUDY,
        "status": status,
        "reason": reason,
        "assertions": assertions,
        "source_session_digest": digest(source_id) if source_id else None,
        "fork_session_digest": digest(fork_id) if fork_id else None,
        "cleanup": "complete" if cleanup_complete else "incomplete",
    }
    if args.result:
        args.result.write_text(json.dumps(result, indent=2))
        os.chmod(args.result, 0o600)
    else:
        print(json.dumps(result, indent=2))
    return 0 if status == "pass" else 1


if __name__ == "__main__":
    sys.exit(main())
