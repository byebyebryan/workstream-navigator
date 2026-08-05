"""Shared disposable-state helpers for OpenCode decision studies."""

from __future__ import annotations

import os
import shutil
import subprocess
from pathlib import Path


def isolated_environment(root: Path) -> dict[str, str]:
    """Return an OpenCode environment rooted entirely below ``root``."""

    env = os.environ.copy()
    source_data_home = env.get("XDG_DATA_HOME")
    for variable, suffix in (
        ("XDG_CONFIG_HOME", "xdg-config"),
        ("XDG_DATA_HOME", "xdg-data"),
        ("XDG_CACHE_HOME", "xdg-cache"),
        ("XDG_STATE_HOME", "xdg-state"),
    ):
        directory = root / suffix
        directory.mkdir(parents=True, exist_ok=True)
        env[variable] = str(directory)
    source_auth = (
        Path(source_data_home or (Path.home() / ".local" / "share"))
        / "opencode"
        / "auth.json"
    )
    target_auth = Path(env["XDG_DATA_HOME"]) / "opencode" / "auth.json"
    if source_auth.exists() and source_auth != target_auth:
        target_auth.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(source_auth, target_auth)
        target_auth.chmod(0o600)
    return env


def opencode_db(env: dict[str, str]) -> str:
    out = subprocess.run(
        ["opencode", "db", "path"],
        capture_output=True,
        text=True,
        check=True,
        env=env,
    ).stdout.strip()
    return out


def environment_for_directory(env: dict[str, str], directory: Path) -> dict[str, str]:
    """Keep OpenCode's project discovery aligned with subprocess ``cwd``."""

    result = env.copy()
    result["PWD"] = str(directory)
    return result


def remove_root(root: Path | None) -> bool:
    if root is None:
        return True
    shutil.rmtree(root, ignore_errors=True)
    return not root.exists()


def all_required(assertions: dict[str, bool], *names: str) -> bool:
    """Return true only when every named assertion is true."""

    return all(assertions[name] for name in names)
