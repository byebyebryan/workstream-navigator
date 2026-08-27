#!/usr/bin/env python3
"""Exercise D17 controlled account-shell wrapper startup without providers.

The study compares a normal interactive non-login Bash/Zsh startup against a
wrapper startup that replays the controlled user RC file exactly once and then
installs provider functions.  It uses fresh private tmux servers and temporary
homes only.  Raw shell state, paths, and output stay inside the temporary root;
the result records bounded booleans and tool versions.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shlex
import shutil
import subprocess
import tempfile
import time
from pathlib import Path
from typing import Any, Final

STUDY: Final = "d17-account-shell-wrapper"
CONTRACT: Final = "account-shell-wrapper-parity-v1"
ROOT_PREFIX: Final = "wsnav-d17-account-shell."
WAIT_SECONDS: Final = 4.0
POLL_SECONDS: Final = 0.03


class StudyFailure(RuntimeError):
    """The controlled startup contradicted the candidate contract."""


class StudyBlocked(RuntimeError):
    """The disposable study could not start safely."""


BASH_USER_RC: Final = r"""printf x >> "${WSNAV_RC_COUNT:?}"
export WSNAV_USER_MARKER=present
PS1='wsnav-user-prompt'
set -o noclobber
alias wsnav_user_alias=':'
alias codex=':'
opencode() { :; }
wsnav_user_function() { :; }
"""

BASH_ABORT_RC: Final = "return 1\n"

BASH_WRAPPER: Final = r"""if shopt -q login_shell; then
    export WSNAV_WRAPPER_REFUSED=login
    return 64
fi
if ! source "${WSNAV_ORIGINAL_HOME:?}/.bashrc"; then
    export WSNAV_WRAPPER_REFUSED=startup_abort
    return 64
fi
unalias codex opencode 2>/dev/null || true
unset -f codex opencode 2>/dev/null || true
codex() { :; }
opencode() { :; }
export WSNAV_WRAPPER_ACTIVE=1
"""

BASH_PROBE: Final = r"""wsnav_probe_kind() {
    local wsnav_name="$1"
    local wsnav_kind
    wsnav_kind="$(type -t "$wsnav_name" 2>/dev/null || true)"
    printf '%s' "${wsnav_kind:-none}"
}
{
    printf 'user_marker=%s\n' "${WSNAV_USER_MARKER-unset}"
    printf 'home_matches_original=%s\n' "$([[ "$HOME" == "$WSNAV_ORIGINAL_HOME" ]] && printf true || printf false)"
    printf 'prompt_ready=%s\n' "$([[ -n "${PS1-}" ]] && printf true || printf false)"
    printf 'noclobber=%s\n' "$([[ -o noclobber ]] && printf true || printf false)"
    printf 'user_alias_kind=%s\n' "$(wsnav_probe_kind wsnav_user_alias)"
    printf 'user_function_kind=%s\n' "$(wsnav_probe_kind wsnav_user_function)"
    printf 'codex_kind=%s\n' "$(wsnav_probe_kind codex)"
    printf 'opencode_kind=%s\n' "$(wsnav_probe_kind opencode)"
    printf 'wrapper_active=%s\n' "${WSNAV_WRAPPER_ACTIVE-0}"
    printf 'wrapper_refused=%s\n' "${WSNAV_WRAPPER_REFUSED-none}"
} > "${WSNAV_PROBE_OUT:?}"
"""

ZSH_USER_RC: Final = r"""print -n -- x >> "${WSNAV_RC_COUNT:?}"
export WSNAV_USER_MARKER=present
PS1='wsnav-user-prompt'
setopt noclobber
alias wsnav_user_alias=':'
alias codex=':'
opencode() { :; }
wsnav_user_function() { :; }
"""

ZSH_ABORT_RC: Final = "return 1\n"

ZSH_WRAPPER: Final = r"""if [[ -o login ]]; then
    export WSNAV_WRAPPER_REFUSED=login
    return 64
fi
export ZDOTDIR="${WSNAV_ORIGINAL_ZDOTDIR:?}"
if ! source "${ZDOTDIR}/.zshrc"; then
    export WSNAV_WRAPPER_REFUSED=startup_abort
    return 64
fi
unalias codex opencode 2>/dev/null || true
unfunction codex opencode 2>/dev/null || true
codex() { :; }
opencode() { :; }
export WSNAV_WRAPPER_ACTIVE=1
"""

ZSH_PROBE: Final = r"""wsnav_probe_alias_kind() {
    if (( $+aliases[$1] )); then
        print -n -- alias
    elif (( $+functions[$1] )); then
        print -n -- function
    else
        print -n -- none
    fi
}
{
    print -r -- "user_marker=${WSNAV_USER_MARKER-unset}"
    [[ "$HOME" == "$WSNAV_ORIGINAL_HOME" ]] && print -r -- 'home_matches_original=true' || print -r -- 'home_matches_original=false'
    [[ "$ZDOTDIR" == "$WSNAV_ORIGINAL_ZDOTDIR" ]] && print -r -- 'zdotdir_matches_original=true' || print -r -- 'zdotdir_matches_original=false'
    [[ -n "${PS1-}" ]] && print -r -- 'prompt_ready=true' || print -r -- 'prompt_ready=false'
    [[ -o noclobber ]] && print -r -- 'noclobber=true' || print -r -- 'noclobber=false'
    print -r -- "user_alias_kind=$(wsnav_probe_alias_kind wsnav_user_alias)"
    print -r -- "user_function_kind=$(wsnav_probe_alias_kind wsnav_user_function)"
    print -r -- "codex_kind=$(wsnav_probe_alias_kind codex)"
    print -r -- "opencode_kind=$(wsnav_probe_alias_kind opencode)"
    print -r -- "wrapper_active=${WSNAV_WRAPPER_ACTIVE-0}"
    print -r -- "wrapper_refused=${WSNAV_WRAPPER_REFUSED-none}"
} > "${WSNAV_PROBE_OUT:?}"
"""


def run(
    arguments: list[str], *, environment: dict[str, str] | None = None
) -> subprocess.CompletedProcess[str]:
    try:
        return subprocess.run(
            arguments,
            capture_output=True,
            check=False,
            env=environment,
            text=True,
            timeout=WAIT_SECONDS,
        )
    except subprocess.TimeoutExpired as error:
        raise StudyFailure("subprocess-timeout") from error
    except OSError as error:
        raise StudyBlocked("required-subprocess-unavailable") from error


def private_tmux(
    socket: Path,
    configuration: Path,
    *arguments: str,
    environment: dict[str, str] | None = None,
    check: bool = True,
) -> subprocess.CompletedProcess[str]:
    tmux_environment = dict(os.environ if environment is None else environment)
    tmux_environment.pop("TMUX", None)
    result = run(
        ["tmux", "-f", str(configuration), "-S", str(socket), *arguments],
        environment=tmux_environment,
    )
    if check and result.returncode != 0:
        raise StudyFailure("private-tmux-command-failed")
    return result


def ordinary_tmux_fingerprint() -> str:
    environment = dict(os.environ)
    environment.pop("TMUX", None)
    result = run(
        [
            "tmux",
            "list-sessions",
            "-F",
            "#{session_name}:#{session_created}",
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
        raise StudyBlocked(f"{command}-version-unavailable")
    value = result.stdout.splitlines()[0].strip()
    if len(value) > 160:
        raise StudyFailure(f"{command}-version-malformed")
    return value


def write_private(path: Path, content: str, mode: int = 0o600) -> None:
    path.write_text(content, encoding="utf-8")
    path.chmod(mode)


def write_tmux_config(path: Path) -> None:
    write_private(
        path,
        'set -g default-terminal "tmux-256color"\n'
        "set -g status off\n"
        "set -g mouse off\n"
        "set -g escape-time 0\n",
    )


def wait_until(predicate: Any, reason: str) -> None:
    deadline = time.monotonic() + WAIT_SECONDS
    while time.monotonic() < deadline:
        if predicate():
            return
        time.sleep(POLL_SECONDS)
    raise StudyFailure(reason)


def read_probe(path: Path) -> dict[str, str]:
    if not path.is_file():
        raise StudyFailure("probe-record-missing")
    values: dict[str, str] = {}
    allowed = {
        "user_marker",
        "home_matches_original",
        "zdotdir_matches_original",
        "prompt_ready",
        "noclobber",
        "user_alias_kind",
        "user_function_kind",
        "codex_kind",
        "opencode_kind",
        "wrapper_active",
        "wrapper_refused",
    }
    for line in path.read_text(encoding="utf-8").splitlines():
        key, separator, value = line.partition("=")
        if not separator or key not in allowed or key in values or len(value) > 80:
            raise StudyFailure("probe-record-malformed")
        values[key] = value
    if not values:
        raise StudyFailure("probe-record-empty")
    return values


def count_is_one(path: Path) -> bool:
    return path.is_file() and path.read_text(encoding="utf-8") == "x"


def start_case(
    root: Path,
    configuration: Path,
    name: str,
    command: list[str],
    environment: dict[str, str],
) -> dict[str, str]:
    socket = root / f"{name}.sock"
    shell_command = shlex.join(
        ["env", *[f"{key}={value}" for key, value in environment.items()], *command]
    )
    private_tmux(
        socket,
        configuration,
        "new-session",
        "-d",
        "-s",
        name,
        "-n",
        "shell",
        "-c",
        str(root),
        shell_command,
        environment=environment,
    )
    private_tmux(
        socket,
        configuration,
        "send-keys",
        "-t",
        f"{name}:0.0",
        "-l",
        'source "$WSNAV_PROBE_SCRIPT"',
    )
    private_tmux(
        socket,
        configuration,
        "send-keys",
        "-t",
        f"{name}:0.0",
        "C-m",
    )
    probe_path = Path(environment["WSNAV_PROBE_OUT"])
    wait_until(probe_path.is_file, "probe-record-timeout")
    try:
        return read_probe(probe_path)
    finally:
        private_tmux(socket, configuration, "kill-server", check=False)


def shell_files(root: Path, shell: str, abort: bool) -> tuple[Path, Path, Path, Path]:
    home = root / f"{shell}-home"
    home.mkdir(mode=0o700)
    user_zdotdir = root / f"{shell}-zdotdir"
    user_zdotdir.mkdir(mode=0o700)
    wrapper_zdotdir = root / f"{shell}-wrapper-zdotdir"
    wrapper_zdotdir.mkdir(mode=0o700)
    if shell == "bash":
        write_private(home / ".bashrc", BASH_ABORT_RC if abort else BASH_USER_RC)
        wrapper = root / "bash-wrapper"
        write_private(wrapper, BASH_WRAPPER)
        probe = root / "bash-probe"
        write_private(probe, BASH_PROBE)
    elif shell == "zsh":
        write_private(user_zdotdir / ".zshrc", ZSH_ABORT_RC if abort else ZSH_USER_RC)
        write_private(wrapper_zdotdir / ".zshrc", ZSH_WRAPPER)
        wrapper = wrapper_zdotdir / ".zshrc"
        probe = root / "zsh-probe"
        write_private(probe, ZSH_PROBE)
    else:
        raise StudyFailure("shell-unsupported")
    return home, user_zdotdir, wrapper, probe


def launch_environment(
    home: Path,
    user_zdotdir: Path,
    probe: Path,
    output: Path,
    count: Path,
) -> dict[str, str]:
    environment = dict(os.environ)
    environment.update(
        {
            "HOME": str(home),
            "WSNAV_ORIGINAL_HOME": str(home),
            "WSNAV_ORIGINAL_ZDOTDIR": str(user_zdotdir),
            "WSNAV_PROBE_OUT": str(output),
            "WSNAV_RC_COUNT": str(count),
            "WSNAV_PROBE_SCRIPT": str(probe),
        }
    )
    return environment


def nonlogin_command(
    shell: str, wrapper: bool, wrapper_path: Path, probe: Path
) -> list[str]:
    if shell == "bash":
        command = ["bash", "--noprofile"]
        if wrapper:
            command.extend(["--rcfile", str(wrapper_path)])
        command.append("-i")
        return command
    if shell == "zsh":
        return ["zsh", "-i"]
    raise StudyFailure("shell-unsupported")


def login_command(shell: str, wrapper_path: Path, probe: Path) -> list[str]:
    if shell == "bash":
        return [
            "bash",
            "--noprofile",
            "--login",
            "--rcfile",
            str(wrapper_path),
            "-i",
        ]
    if shell == "zsh":
        return ["zsh", "-l", "-i"]
    raise StudyFailure("shell-unsupported")


def run_parity_case(
    root: Path, configuration: Path, shell: str
) -> dict[str, bool | str]:
    baseline_root = root / f"{shell}-baseline"
    wrapped_root = root / f"{shell}-wrapped"
    baseline_root.mkdir(mode=0o700)
    wrapped_root.mkdir(mode=0o700)
    baseline_home, baseline_zdotdir, _, baseline_probe = shell_files(
        baseline_root, shell, False
    )
    wrapped_home, wrapped_zdotdir, wrapper, wrapped_probe = shell_files(
        wrapped_root, shell, False
    )
    baseline_count = baseline_root / f"{shell}-count"
    wrapped_count = wrapped_root / f"{shell}-count"
    baseline_environment = launch_environment(
        baseline_home,
        baseline_zdotdir,
        baseline_probe,
        baseline_root / "probe",
        baseline_count,
    )
    wrapped_environment = launch_environment(
        wrapped_home,
        wrapped_zdotdir,
        wrapped_probe,
        wrapped_root / "probe",
        wrapped_count,
    )
    if shell == "zsh":
        baseline_environment["ZDOTDIR"] = str(baseline_zdotdir)
        wrapped_environment["ZDOTDIR"] = str(wrapper.parent)
    baseline = start_case(
        root,
        configuration,
        f"{shell}-baseline",
        nonlogin_command(shell, False, wrapper, baseline_probe),
        baseline_environment,
    )
    wrapped = start_case(
        root,
        configuration,
        f"{shell}-wrapped",
        nonlogin_command(shell, True, wrapper, wrapped_probe),
        wrapped_environment,
    )
    common = (
        "user_marker",
        "home_matches_original",
        "prompt_ready",
        "noclobber",
        "user_alias_kind",
        "user_function_kind",
    )
    if shell == "zsh":
        common = (*common, "zdotdir_matches_original")
    result = {
        "user_rc_once_in_baseline": count_is_one(baseline_count),
        "user_rc_once_in_wrapper": count_is_one(wrapped_count),
        "observable_user_state_matches_baseline": all(
            baseline.get(key) == wrapped.get(key) for key in common
        ),
        "baseline_keeps_user_codex_alias": baseline.get("codex_kind") == "alias",
        "baseline_keeps_user_opencode_function": baseline.get("opencode_kind")
        == "function",
        "wrapper_replaces_codex_alias": wrapped.get("codex_kind") == "function",
        "wrapper_replaces_opencode_function": wrapped.get("opencode_kind")
        == "function",
        "wrapper_marks_only_wrapped_startup": baseline.get("wrapper_active") == "0"
        and wrapped.get("wrapper_active") == "1",
        "wrapper_does_not_refuse_normal_startup": wrapped.get("wrapper_refused")
        == "none",
        "wrapped_startup_observation": ":".join(
            wrapped.get(key, "missing")
            for key in (
                "wrapper_active",
                "wrapper_refused",
                "codex_kind",
                "opencode_kind",
            )
        ),
    }
    return result


def run_refusal_case(
    root: Path, configuration: Path, shell: str, reason: str
) -> dict[str, bool | str]:
    case_root = root / f"{shell}-{reason}"
    case_root.mkdir(mode=0o700)
    home, user_zdotdir, wrapper, probe = shell_files(
        case_root, shell, reason == "startup_abort"
    )
    count = case_root / f"{shell}-count"
    environment = launch_environment(
        home, user_zdotdir, probe, case_root / "probe", count
    )
    if shell == "zsh":
        environment["ZDOTDIR"] = str(wrapper.parent)
    observed = start_case(
        root,
        configuration,
        f"{shell}-{reason}",
        (
            login_command(shell, wrapper, probe)
            if reason == "login"
            else nonlogin_command(shell, True, wrapper, probe)
        ),
        environment,
    )
    if shell == "bash" and reason == "login":
        result = {
            "bash_login_does_not_load_rcfile": observed.get("wrapper_active") == "0"
            and observed.get("wrapper_refused") == "none",
            "provider_functions_not_installed": observed.get("wrapper_active") != "1",
            "launcher_preflight_required": True,
            "refusal_observation": ":".join(
                observed.get(key, "missing")
                for key in (
                    "wrapper_active",
                    "wrapper_refused",
                    "codex_kind",
                    "opencode_kind",
                )
            ),
        }
        return result
    result = {
        "reason_is_reported": observed.get("wrapper_refused") == reason,
        "provider_functions_not_installed": observed.get("wrapper_active") != "1",
        "aborted_user_rc_not_counted": reason != "startup_abort" or not count.exists(),
        "refusal_observation": ":".join(
            observed.get(key, "missing")
            for key in (
                "wrapper_active",
                "wrapper_refused",
                "codex_kind",
                "opencode_kind",
            )
        ),
    }
    return result


def write_result(path: Path | None, result: dict[str, Any]) -> None:
    rendered = json.dumps(result, indent=2, sort_keys=True) + "\n"
    if path is None:
        print(rendered, end="")
        return
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(rendered, encoding="utf-8")
    path.chmod(0o600)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--result", type=Path)
    options = parser.parse_args()
    root: Path | None = None
    configuration: Path | None = None
    before_fingerprint = ""
    baseline_established = False
    cases: dict[str, dict[str, bool | str]] = {}
    assertions: dict[str, bool] = {}
    versions: dict[str, str] = {}
    status = "pass"
    reason = "account-shell-wrapper-parity-observed"
    try:
        if any(shutil.which(command) is None for command in ("bash", "tmux", "zsh")):
            raise StudyBlocked("required-command-unavailable")
        versions = {
            "bash_version": tool_version("bash", "--version"),
            "tmux_version": tool_version("tmux", "-V"),
            "zsh_version": tool_version("zsh", "--version"),
        }
        before_fingerprint = ordinary_tmux_fingerprint()
        baseline_established = True
        root = Path(tempfile.mkdtemp(prefix=ROOT_PREFIX))
        root.chmod(0o700)
        configuration = root / "tmux.conf"
        write_tmux_config(configuration)
        for shell in ("bash", "zsh"):
            cases[f"{shell}_parity"] = run_parity_case(root, configuration, shell)
            cases[f"{shell}_login_refusal"] = run_refusal_case(
                root, configuration, shell, "login"
            )
            cases[f"{shell}_startup_abort"] = run_refusal_case(
                root, configuration, shell, "startup_abort"
            )
        assertions = {
            "all_case_assertions_pass": all(
                value
                for case in cases.values()
                for value in case.values()
                if isinstance(value, bool)
            ),
            "private_tmux_only": True,
            "temporary_root_mode_0700": root.stat().st_mode & 0o777 == 0o700,
        }
        if not all(assertions.values()):
            raise StudyFailure("aggregate-assertion-failed")
    except StudyBlocked as error:
        status = "blocked"
        reason = str(error)
    except StudyFailure as error:
        status = "falsified"
        reason = str(error)
    finally:
        if root is not None:
            for socket in root.glob("*.sock"):
                if configuration is not None:
                    private_tmux(socket, configuration, "kill-server", check=False)
            try:
                shutil.rmtree(root)
            except OSError:
                status = "falsified"
                reason = "temporary-root-cleanup-failed"
        assertions["cleanup_complete"] = root is None or not root.exists()
        if baseline_established:
            assertions["ordinary_tmux_unchanged"] = (
                before_fingerprint == ordinary_tmux_fingerprint()
            )
        else:
            assertions["ordinary_tmux_unchanged"] = False
        if not assertions["cleanup_complete"]:
            status = "falsified"
            reason = "temporary-root-cleanup-failed"
        if baseline_established and not assertions["ordinary_tmux_unchanged"]:
            status = "falsified"
            reason = "ordinary-tmux-changed"
    write_result(
        options.result,
        {
            "study": STUDY,
            "contract_fingerprint": CONTRACT,
            "status": status,
            "reason": reason,
            "environment": versions,
            "cases": cases,
            "assertions": assertions,
        },
    )
    return 0 if status == "pass" else 1


if __name__ == "__main__":
    raise SystemExit(main())
