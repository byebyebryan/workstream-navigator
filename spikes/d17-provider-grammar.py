#!/usr/bin/env python3
"""Validate the pinned D17 fresh-TUI command grammars without a provider run.

The probe reads only the installed ``--version`` and ``--help`` surfaces for
Codex and OpenCode.  It then executes a deterministic grammar matrix in-process:
fresh interactive shapes become broker candidates, a very small explicitly
enumerated information/auth set remains unmanaged, and all identity-changing or
ambiguous forms refuse before a broker reservation could exist.

No provider is started and only booleans, route labels, argument digests, and
installed versions are retained in the result fixture.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Final

STUDY: Final = "d17-provider-grammar"
CONTRACT: Final = "d17-fresh-tui-grammar-v1"
COMMAND_TIMEOUT_SECONDS: Final = 4.0
MAX_ARGUMENTS: Final = 16
MAX_ARGUMENT_BYTES: Final = 160
MAX_REPLAY_LIMIT: Final = 10_000
VALUE_PATTERN: Final = re.compile(r"[A-Za-z0-9._/:=-]+")
AGENT_PATTERN: Final = re.compile(r"[A-Za-z0-9._:-]+")


class SpikeFailure(RuntimeError):
    """A provider surface or grammar result contradicts the contract."""


@dataclass(frozen=True)
class Classification:
    route: str
    normalized: tuple[str, ...] = ()


def write_result(path: Path, value: dict[str, object]) -> None:
    encoded = (json.dumps(value, sort_keys=True, indent=2) + "\n").encode("utf-8")
    path.parent.mkdir(parents=True, exist_ok=True)
    descriptor = path.open("wb")
    try:
        descriptor.write(encoded)
        descriptor.flush()
    finally:
        descriptor.close()
    path.chmod(0o600)


def command_output(command: str, argument: str) -> str:
    try:
        result = subprocess.run(
            [command, argument],
            capture_output=True,
            check=False,
            text=True,
            timeout=COMMAND_TIMEOUT_SECONDS,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        raise SpikeFailure(f"{command}-unavailable") from error
    output = result.stdout + result.stderr
    if result.returncode != 0 or not output.strip():
        raise SpikeFailure(f"{command}-{argument.lstrip('-')}-unavailable")
    if len(output.encode("utf-8")) > 256 * 1024:
        raise SpikeFailure(f"{command}-{argument.lstrip('-')}-oversized")
    return output


def version(command: str) -> str:
    output = command_output(command, "--version")
    first_line = output.splitlines()[0].strip()
    if len(first_line) > 160:
        raise SpikeFailure(f"{command}-version-malformed")
    return first_line


def ensure_safe_value(value: str, *, agent: bool = False) -> str:
    pattern = AGENT_PATTERN if agent else VALUE_PATTERN
    if (
        not value
        or len(value.encode("utf-8")) > MAX_ARGUMENT_BYTES
        or pattern.fullmatch(value) is None
        or value.lower().startswith(
            ("sk-", "sk_", "token-", "bearer-", "ghp_", "xoxb-")
        )
    ):
        raise ValueError("unsafe-value")
    return value


def check_argument_shape(arguments: tuple[str, ...]) -> None:
    if len(arguments) > MAX_ARGUMENTS:
        raise ValueError("too-many-arguments")
    for argument in arguments:
        if not argument or len(argument.encode("utf-8")) > MAX_ARGUMENT_BYTES:
            raise ValueError("unsafe-argument")
        if "\x00" in argument or any(character.isspace() for character in argument):
            raise ValueError("unsafe-argument")
        if argument == "--" or "=" in argument:
            raise ValueError("ambiguous-argument")


def classify_codex(arguments: tuple[str, ...]) -> Classification:
    check_argument_shape(arguments)
    if arguments in (("-h",), ("--help",), ("-V",), ("--version",), ("login",)):
        return Classification("unmanaged")

    allowed_values = {
        "-m": "--model",
        "--model": "--model",
        "--local-provider": "--local-provider",
        "-s": "--sandbox",
        "--sandbox": "--sandbox",
        "-a": "--ask-for-approval",
        "--ask-for-approval": "--ask-for-approval",
    }
    allowed_flags = {"--oss", "--search", "--no-alt-screen", "--approve-for-me"}
    normalized: list[str] = []
    seen: set[str] = set()
    index = 0
    while index < len(arguments):
        argument = arguments[index]
        if argument in allowed_flags:
            if argument in seen:
                raise ValueError("duplicate-option")
            seen.add(argument)
            normalized.append(argument)
            index += 1
            continue
        canonical = allowed_values.get(argument)
        if canonical is None or index + 1 >= len(arguments):
            raise ValueError("unsupported-codex-shape")
        if canonical in seen:
            raise ValueError("duplicate-option")
        value = ensure_safe_value(arguments[index + 1])
        if canonical == "--local-provider" and value not in ("lmstudio", "ollama"):
            raise ValueError("unsupported-local-provider")
        if canonical == "--sandbox" and value not in (
            "read-only",
            "workspace-write",
            "danger-full-access",
        ):
            raise ValueError("unsupported-sandbox")
        if canonical == "--ask-for-approval" and value not in ("on-request", "never"):
            raise ValueError("unsupported-approval")
        seen.add(canonical)
        normalized.extend((canonical, value))
        index += 2

    if "--local-provider" in seen and "--oss" not in seen:
        raise ValueError("local-provider-requires-oss")
    return Classification("managed-fresh", tuple(normalized))


def classify_opencode(arguments: tuple[str, ...]) -> Classification:
    check_argument_shape(arguments)
    if arguments in (("-h",), ("--help",), ("-v",), ("--version",), ("providers",)):
        return Classification("unmanaged")

    allowed_values = {
        "-m": "--model",
        "--model": "--model",
        "--agent": "--agent",
        "--replay-limit": "--replay-limit",
    }
    allowed_flags = {"--pure", "--auto", "--mini", "--no-replay"}
    normalized: list[str] = []
    seen: set[str] = set()
    index = 0
    while index < len(arguments):
        argument = arguments[index]
        if argument in allowed_flags:
            if argument in seen:
                raise ValueError("duplicate-option")
            seen.add(argument)
            normalized.append(argument)
            index += 1
            continue
        canonical = allowed_values.get(argument)
        if canonical is None or index + 1 >= len(arguments):
            raise ValueError("unsupported-opencode-shape")
        if canonical in seen:
            raise ValueError("duplicate-option")
        value = arguments[index + 1]
        if canonical == "--agent":
            value = ensure_safe_value(value, agent=True)
        elif canonical == "--replay-limit":
            if (
                not value.isascii()
                or not value.isdecimal()
                or int(value) > MAX_REPLAY_LIMIT
            ):
                raise ValueError("unsupported-replay-limit")
        else:
            value = ensure_safe_value(value)
        seen.add(canonical)
        normalized.extend((canonical, value))
        index += 2
    return Classification("managed-fresh", tuple(normalized))


def classify(provider: str, arguments: tuple[str, ...]) -> Classification:
    if provider == "codex":
        return classify_codex(arguments)
    if provider == "opencode":
        return classify_opencode(arguments)
    raise SpikeFailure("unknown-provider")


def digest(arguments: tuple[str, ...]) -> str:
    return hashlib.sha256(
        json.dumps(arguments, ensure_ascii=True, separators=(",", ":")).encode("ascii")
    ).hexdigest()


def accepted(provider: str, arguments: tuple[str, ...]) -> bool:
    return classify(provider, arguments).route == "managed-fresh"


def unmanaged(provider: str, arguments: tuple[str, ...]) -> bool:
    return classify(provider, arguments).route == "unmanaged"


def refused(provider: str, arguments: tuple[str, ...]) -> bool:
    try:
        classify(provider, arguments)
    except ValueError:
        return True
    return False


def run_probe() -> dict[str, object]:
    codex_help = command_output("codex", "--help")
    opencode_help = command_output("opencode", "--help")
    assertions = {
        "codex_help_exposes_native_and_rejected_boundaries": all(
            fragment in codex_help
            for fragment in ("Codex CLI", "resume", "--remote", "--cd", "--profile")
        ),
        "opencode_help_exposes_native_and_rejected_boundaries": all(
            fragment in opencode_help
            for fragment in (
                "opencode [project]",
                "attach <url>",
                "--session",
                "--port",
                "--hostname",
            )
        ),
        "codex_empty_is_managed_fresh": accepted("codex", ()),
        "codex_safe_native_options_are_managed": accepted(
            "codex",
            ("--model", "gpt-5.6", "--sandbox", "workspace-write", "--no-alt-screen"),
        ),
        "codex_local_provider_requires_explicit_oss": accepted(
            "codex", ("--oss", "--local-provider", "ollama")
        )
        and refused("codex", ("--local-provider", "ollama")),
        "codex_information_and_login_are_unmanaged": all(
            unmanaged("codex", arguments)
            for arguments in (("--help",), ("--version",), ("login",))
        ),
        "codex_identity_session_profile_and_prompt_forms_refuse": all(
            refused("codex", arguments)
            for arguments in (
                ("resume", "--last"),
                ("fork", "--last"),
                ("--remote", "ws://127.0.0.1:8080"),
                ("--profile", "other"),
                ("--cd", "elsewhere"),
                ("--add-dir", "elsewhere"),
                ("--config", "model=other"),
                ("--image", "image.png"),
                ("--dangerously-bypass-approvals-and-sandbox",),
                ("initial-prompt",),
            )
        ),
        "codex_ambiguous_or_secret_like_forms_refuse": all(
            refused("codex", arguments)
            for arguments in (
                ("--model=gpt-5.6",),
                ("--model", "token value"),
                ("--model", "sk-secret"),
                ("--model", "gpt-5.6", "--model", "other"),
            )
        ),
        "opencode_empty_is_managed_fresh": accepted("opencode", ()),
        "opencode_safe_native_options_are_managed": accepted(
            "opencode",
            (
                "--model",
                "openai/gpt-5.6",
                "--agent",
                "build",
                "--mini",
                "--replay-limit",
                "128",
            ),
        ),
        "opencode_information_and_auth_are_unmanaged": all(
            unmanaged("opencode", arguments)
            for arguments in (("--help",), ("--version",), ("providers",))
        ),
        "opencode_identity_session_server_and_prompt_forms_refuse": all(
            refused("opencode", arguments)
            for arguments in (
                ("project-path",),
                ("attach", "http://127.0.0.1:4096"),
                ("--continue",),
                ("--session", "session-id"),
                ("--fork",),
                ("--port", "4096"),
                ("--hostname", "0.0.0.0"),
                ("--mdns",),
                ("--cors", "http://example.test"),
                ("--prompt", "initial-prompt"),
                ("run", "initial-prompt"),
            )
        ),
        "opencode_ambiguous_or_oversized_forms_refuse": all(
            refused("opencode", arguments)
            for arguments in (
                ("--model=openai/gpt-5.6",),
                ("--agent", "agent name"),
                ("--replay-limit", "10001"),
                ("--model", "openai/gpt-5.6", "--model", "other/model"),
                tuple("--pure" for _ in range(MAX_ARGUMENTS + 1)),
            )
        ),
        "managed_routes_bind_normalized_argv_digest": (
            classify("codex", ("-m", "gpt-5.6", "--oss")).normalized
            == ("--model", "gpt-5.6", "--oss")
            and digest(classify("codex", ("-m", "gpt-5.6", "--oss")).normalized)
            == digest(classify("codex", ("--model", "gpt-5.6", "--oss")).normalized)
            and classify("opencode", ("-m", "openai/gpt-5.6", "--mini")).normalized
            == ("--model", "openai/gpt-5.6", "--mini")
            and len(
                digest(
                    classify("opencode", ("-m", "openai/gpt-5.6", "--mini")).normalized
                )
            )
            == 64
        ),
    }
    assertions["all_case_assertions_pass"] = all(assertions.values())
    return {
        "contract": CONTRACT,
        "status": "pass" if assertions["all_case_assertions_pass"] else "falsified",
        "reason": "pinned-fresh-tui-grammar-observed",
        "providers": {"codex": version("codex"), "opencode": version("opencode")},
        "assertions": assertions,
    }


def parse_arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--result", type=Path, required=True)
    return parser.parse_args()


def main() -> int:
    arguments = parse_arguments()
    result = run_probe()
    write_result(arguments.result, result)
    return 0 if result["status"] == "pass" else 1


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except SpikeFailure as error:
        print(f"{STUDY}: {error}", file=sys.stderr)
        raise SystemExit(1) from error
