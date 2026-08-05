#!/usr/bin/env python3
"""Probe concurrent opencode runtimes sharing the global SQLite database.

Runs several independent `opencode run` processes against one shared
state database and checks for integrity errors, session visibility across
runtimes, and lost event rows. Uses disposable projects and sessions only;
never touches other opencode sessions or tmux.
"""

from __future__ import annotations

import argparse
import json
import os
import sqlite3
import subprocess
import sys
import tempfile
from pathlib import Path

from opencode_support import (
    environment_for_directory,
    isolated_environment,
    remove_root,
)
from opencode_support import opencode_db as support_opencode_db

STUDY = "opencode-shared-db-concurrency"
MODEL = "opencode-go/deepseek-v4-flash"
RUN_COUNT = 4
MARKER = "WSNAV_OC_CONC"
PROMPTS = [
    f"Reply with the exact token {MARKER}_{i} and nothing else. "
    "Do not use tools, inspect files, or make changes."
    for i in range(RUN_COUNT)
]


class StudyFailure(RuntimeError):
    pass


class StudyBlocked(RuntimeError):
    pass


def run_one(
    prompt: str,
    cwd: Path,
    env: dict[str, str],
    session_id: str | None = None,
) -> subprocess.Popen[str]:
    args = ["opencode", "run", "--model", MODEL, "--format", "json"]
    if session_id is not None:
        args += ["--session", session_id]
    args += [prompt]
    return subprocess.Popen(
        args,
        cwd=cwd,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        env=environment_for_directory(env, cwd),
    )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--result", type=Path)
    args = parser.parse_args()

    assertions = {
        "all_concurrent_runs_completed": False,
        "db_integrity_check_passes": False,
        "all_sessions_visible_in_shared_db": False,
        "each_run_has_distinct_session": False,
    }
    root: Path | None = None
    status = "blocked"
    reason = "harness-early-exit"
    cleanup_complete = True
    session_ids: list[str] = []
    try:
        root = Path(tempfile.mkdtemp(prefix="wsnav-occonc."))
        env = isolated_environment(root)
        project = root / "project"
        project.mkdir()

        db_before = support_opencode_db(env)

        # Launch all runs concurrently against the same disposable DB.
        procs = [run_one(prompt, project, env) for prompt in PROMPTS]
        outs: list[str] = []
        all_ok = True
        for proc in procs:
            try:
                out, _ = proc.communicate(timeout=180)
            except subprocess.TimeoutExpired:
                proc.kill()
                out, _ = proc.communicate()
                all_ok = False
            outs.append(out)
            if proc.returncode != 0:
                all_ok = False
        assertions["all_concurrent_runs_completed"] = all_ok

        for out in outs:
            for line in out.splitlines():
                try:
                    ev = json.loads(line)
                except json.JSONDecodeError:
                    continue
                if ev.get("type") == "step_start" and ev.get("sessionID"):
                    sid = ev["sessionID"]
                    if sid not in session_ids:
                        session_ids.append(sid)
        assertions["each_run_has_distinct_session"] = len(set(session_ids)) == RUN_COUNT

        # Integrity check on the shared DB.
        conn = sqlite3.connect(db_before)
        result = conn.execute("pragma integrity_check").fetchone()
        conn.close()
        assertions["db_integrity_check_passes"] = (
            result is not None and result[0] == "ok"
        )

        # All sessions visible in the shared DB.
        conn = sqlite3.connect(db_before)
        rows = conn.execute(
            "select id from session where id in ({})".format(
                ",".join("?" * len(session_ids))
            ),
            session_ids,
        ).fetchall()
        conn.close()
        visible = {r[0] for r in rows}
        assertions["all_sessions_visible_in_shared_db"] = set(session_ids) == visible

        if not all(assertions.values()):
            raise StudyFailure("shared-db concurrency assertions incomplete")
        status = "pass"
        reason = "shared-db-concurrency-confirmed"
    except StudyBlocked as error:
        status, reason = "blocked", str(error)
    except StudyFailure as error:
        status, reason = "falsified", str(error)
    except (subprocess.TimeoutExpired, OSError, sqlite3.Error) as error:
        status, reason = "blocked", f"harness-error:{type(error).__name__}"
    finally:
        cleanup_complete = remove_root(root)

    result = {
        "study": STUDY,
        "status": status,
        "reason": reason,
        "assertions": assertions,
        "concurrent_runs": RUN_COUNT,
        "observed_session_count": len(set(session_ids)),
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
