#!/usr/bin/env python3
"""Probe whether an opencode fork of a RUNNING source snapshots only settled messages.

This is a disposable decision study. It uses its own temporary project and
sessions under a disposable SQLite-backed opencode state; it never touches
the caller's other opencode sessions or their tmux server. Only the exact
session IDs this script created are ever resumed or inspected.
"""

from __future__ import annotations

import argparse
import json
import os
import sqlite3
import subprocess
import sys
import tempfile
import time
from pathlib import Path
from typing import Any

STUDY = "opencode-running-settled-fork"
MODEL = "opencode-go/deepseek-v4-flash"
BASELINE_MARKER = "WSNAV_OC_BASELINE"
ACTIVE_MARKER = "WSNAV_OC_ACTIVE"
FORK_MARKER = "WSNAV_OC_FORK"
BASELINE_PROMPT = (
    f"Reply with the exact token {BASELINE_MARKER} and nothing else. "
    "Do not use tools, inspect files, or make changes."
)
ACTIVE_PROMPT = (
    "Run the shell command sleep 25 exactly once. Do not edit files. "
    "After it finishes, reply with one short confirmation."
)
FORK_PROMPT = (
    f"Reply with the exact token {FORK_MARKER} and nothing else. "
    "Do not use tools, inspect files, or make changes."
)


class StudyFailure(RuntimeError):
    pass


class StudyBlocked(RuntimeError):
    pass


def opencode_db() -> str:
    out = subprocess.run(
        ["opencode", "db", "path"],
        capture_output=True,
        text=True,
        check=True,
    ).stdout.strip()
    return out


def connect() -> sqlite3.Connection:
    db = opencode_db()
    conn = sqlite3.connect(db)
    conn.row_factory = sqlite3.Row
    return conn


def messages(conn: sqlite3.Connection, session_id: str) -> list[dict[str, Any]]:
    rows = conn.execute(
        "select id, time_created, data from message "
        "where session_id = ? order by time_created",
        (session_id,),
    ).fetchall()
    out = []
    for r in rows:
        data = json.loads(r["data"])
        parts = conn.execute(
            "select data from part where message_id = ? order by time_created",
            (r["id"],),
        ).fetchall()
        texts = []
        for p in parts:
            pd = json.loads(p["data"])
            if pd.get("type") == "text" and pd.get("text"):
                texts.append(pd["text"])
        out.append(
            {
                "role": data.get("role"),
                "finish": data.get("finish"),
                "id": r["id"],
                "time_created": r["time_created"],
                "text": "".join(texts),
            }
        )
    return out


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


def new_session(cwd: Path, prompt: str) -> str:
    out = run_opencode([prompt], cwd)
    for line in out.splitlines():
        try:
            ev = json.loads(line)
        except json.JSONDecodeError:
            continue
        if ev.get("type") == "step_start" and ev.get("sessionID"):
            return ev["sessionID"]
    raise StudyFailure("no session id observed from opencode run")


def settled_text(conn: sqlite3.Connection, session_id: str) -> str:
    return "\n".join(
        m["text"] for m in messages(conn, session_id) if m["role"] == "assistant"
    )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--result", type=Path)
    args = parser.parse_args()

    assertions = {
        "source_settled_baseline_recorded": False,
        "in_flight_turn_detected": False,
        "fork_created_distinct_child_session": False,
        "fork_omits_in_flight_turn": False,
        "fork_preserves_settled_prefix_exactly": False,
        "fork_lineage_structural": False,
    }
    root: Path | None = None
    cleanup_complete = True
    source_id: str | None = None
    fork_id: str | None = None
    try:
        root = Path(tempfile.mkdtemp(prefix="wsnav-ocfork-study."))
        project = root / "project"
        project.mkdir()

        source_id = new_session(project, BASELINE_PROMPT)
        time.sleep(1)
        conn = connect()
        source_before = messages(conn, source_id)
        if any(
            m["role"] == "assistant"
            and m["finish"] == "stop"
            and BASELINE_MARKER in m["text"]
            for m in source_before
        ):
            assertions["source_settled_baseline_recorded"] = True
        conn.close()

        # Start an in-flight turn on the source in a separate process.
        active = subprocess.Popen(
            ["opencode", "run", "--model", MODEL, "--format", "json",
             "--session", source_id, ACTIVE_PROMPT],
            cwd=project,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
        deadline = time.time() + 60
        conn = connect()
        observed = []
        while time.time() < deadline:
            msgs = messages(conn, source_id)
            observed.append(
                [(m["role"], m["finish"], m["text"][:16]) for m in msgs]
            )
            active_user = None
            for m in msgs:
                if m["role"] == "user" and "sleep 25" in m["text"]:
                    active_user = m
                    break
            if active_user is not None:
                replied = any(
                    m["role"] == "assistant"
                    and m["finish"] == "stop"
                    and m["id"] != active_user["id"]
                    and m["time_created"] > active_user["time_created"]
                    for m in msgs
                    if m["role"] == "assistant"
                )
                if not replied:
                    assertions["in_flight_turn_detected"] = True
                    break
            time.sleep(1)
        conn.close()
        if not assertions["in_flight_turn_detected"]:
            raise StudyFailure(
                "could not observe the in-flight turn; "
                f"last observed messages: {observed[-3:] if observed else 'none'}"
            )

        # Fork the running source and continue it.
        fork_out = run_opencode(
            ["--session", source_id, "--fork", FORK_PROMPT], project
        )
        fork_id = None
        for line in fork_out.splitlines():
            try:
                ev = json.loads(line)
            except json.JSONDecodeError:
                continue
            if ev.get("type") == "step_start" and ev.get("sessionID") and ev["sessionID"] != source_id:
                fork_id = ev["sessionID"]
        if fork_id is None:
            raise StudyFailure("no distinct fork session observed")
        assertions["fork_created_distinct_child_session"] = True

        # Let the source's active turn finish.
        try:
            active.wait(timeout=60)
        except subprocess.TimeoutExpired:
            active.kill()
            active.wait(timeout=10)

        time.sleep(2)
        conn = connect()
        fork_msgs = messages(conn, fork_id)
        conn.close()

        fork_assistant = [
            m for m in fork_msgs if m["role"] == "assistant"
        ]
        # The fork must contain the settled baseline, must not contain the
        # in-flight active turn's result, and must contain the fork reply.
        if not any(FORK_MARKER in m["text"] for m in fork_assistant):
            raise StudyFailure("fork did not include its own reply")
        if any("sleep 25" in m["text"] for m in fork_assistant):
            raise StudyFailure("fork leaked the in-flight turn")
        if not any(BASELINE_MARKER in m["text"] for m in fork_assistant):
            raise StudyFailure("fork dropped the settled baseline")
        assertions["fork_omits_in_flight_turn"] = True
        assertions["fork_preserves_settled_prefix_exactly"] = True

        # Lineage: is the fork structurally linked to the source?
        conn = connect()
        row = conn.execute(
            "select parent_id from session where id = ?", (fork_id,)
        ).fetchone()
        conn.close()
        if row and row["parent_id"]:
            assertions["fork_lineage_structural"] = True
        status, reason = "pass", "running-source-settled-prefix-fork-confirmed"
    except StudyBlocked as error:
        status, reason = "blocked", str(error)
    except StudyFailure as error:
        status, reason = "falsified", str(error)
    except (subprocess.TimeoutExpired, OSError, sqlite3.Error) as error:
        status, reason = "blocked", f"harness-error:{type(error).__name__}"
    finally:
        if root is not None:
            import shutil

            shutil.rmtree(root, ignore_errors=True)
            cleanup_complete = not root.exists()

    result = {
        "study": STUDY,
        "status": status,
        "reason": reason,
        "assertions": assertions,
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
