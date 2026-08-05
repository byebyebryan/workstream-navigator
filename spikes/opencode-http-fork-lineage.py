#!/usr/bin/env python3
"""Probe HTTP fork lineage: does POST /session/:id/fork record parent_id / children?"""

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

from opencode_support import (
    environment_for_directory,
    isolated_environment,
    remove_root,
)
from opencode_support import opencode_db as support_opencode_db

STUDY = "opencode-http-fork-lineage"
MODEL = "opencode-go/deepseek-v4-flash"
MARKER = "WSNAV_OC_HTTPFORK"
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


def request(
    url: str,
    env: dict[str, str],
    method: str = "GET",
    body: dict[str, Any] | None = None,
) -> Any:
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(url, data=data, method=method)
    req.add_header("Content-Type", "application/json")
    with urllib.request.urlopen(req, timeout=15) as resp:
        return json.loads(resp.read().decode())


def run_opencode(
    args: list[str], cwd: Path, env: dict[str, str], timeout: int = 180
) -> str:
    proc = subprocess.run(
        ["opencode", "run", "--model", MODEL, "--format", "json", *args],
        capture_output=True,
        text=True,
        timeout=timeout,
        cwd=cwd,
        check=False,
        env=environment_for_directory(env, cwd),
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
        "http_fork_returns_new_session": False,
        "http_fork_parent_id_structural": False,
        "http_fork_in_children_api": False,
        "http_fork_recoverable_from_source": False,
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
        root = Path(tempfile.mkdtemp(prefix="wsnav-ochttpfork."))
        env = isolated_environment(root)
        project = root / "project"
        project.mkdir()

        source_out = run_opencode([PROMPT], project, env)
        source_id = first_session_id(source_out)
        if source_id is None:
            raise StudyFailure("no source session id observed")
        time.sleep(1)

        server = subprocess.Popen(
            ["opencode", "serve", "--port", str(port), "--hostname", "127.0.0.1"],
            cwd=project,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            text=True,
            env=environment_for_directory(env, project),
        )
        base = f"http://127.0.0.1:{port}"
        deadline = time.time() + 30
        ready = False
        while time.time() < deadline:
            try:
                request(f"{base}/session/{source_id}", env)
                ready = True
                break
            except (
                urllib.error.URLError,
                urllib.error.HTTPError,
                json.JSONDecodeError,
            ):
                time.sleep(1)
        if not ready:
            raise StudyBlocked("opencode server did not start in time")

        try:
            forked = request(
                f"{base}/session/{source_id}/fork", env, method="POST", body={}
            )
        except (urllib.error.HTTPError, urllib.error.URLError) as error:
            raise StudyFailure(f"HTTP fork rejected: {error}")

        if isinstance(forked, dict):
            fork_id = forked.get("id")
            if fork_id and fork_id != source_id:
                assertions["http_fork_returns_new_session"] = True

        if fork_id is None:
            raise StudyFailure("HTTP fork did not return a destination id")

        conn = sqlite3.connect(support_opencode_db(env))
        row = conn.execute(
            "select parent_id from session where id = ?", (fork_id,)
        ).fetchone()
        conn.close()
        if row is not None and row[0] is not None:
            assertions["http_fork_parent_id_structural"] = True

        children = request(f"{base}/session/{source_id}/children", env)
        child_ids = [c.get("id") if isinstance(c, dict) else c for c in children]
        if fork_id in child_ids:
            assertions["http_fork_in_children_api"] = True
            assertions["http_fork_recoverable_from_source"] = True

        if not assertions["http_fork_returns_new_session"]:
            raise StudyFailure("HTTP fork did not create a distinct session")
        if (
            assertions["http_fork_parent_id_structural"]
            or assertions["http_fork_in_children_api"]
        ):
            raise StudyFailure("provider HTTP fork lineage became structural")
        status = "pass"
        reason = "http-fork-lineage-absence-confirmed"
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
        cleanup_complete = remove_root(root)

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
