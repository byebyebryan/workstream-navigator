#!/usr/bin/env python3
"""Falsify D17 provider-effect journal ordering without launching a provider.

The disposable model drives Codex no-effect preparation and an in-process fake
OpenCode ``POST /session`` endpoint under a durable synthetic onboarding
journal.  It proves that durable Runtime ownership and the action fence precede
provider effects; known and ambiguous OpenCode POST outcomes take different
recovery paths and neither can issue a second POST.
"""

from __future__ import annotations

import argparse
import http.client
import json
import os
import shutil
import tempfile
import threading
import uuid
from dataclasses import dataclass
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any, Final, Self

STUDY: Final = "d17-provider-effect-journal"
CONTRACT: Final = "provider-effect-journal-v1"
ROOT_PREFIX: Final = "wsnav-d17-provider-effect."
ROOT_MODE: Final = 0o700
FILE_MODE: Final = 0o600
MAX_JOURNAL_BYTES: Final = 4096
HTTP_TIMEOUT_SECONDS: Final = 2.0


class SpikeFailure(RuntimeError):
    """A fake-effect result contradicted the D17 contract."""


class JournalRefused(RuntimeError):
    """The exact journal/capability phase does not permit an action."""


@dataclass
class EndpointState:
    mode: str
    post_count: int = 0


def write_private(path: Path, value: dict[str, Any]) -> None:
    encoded = (json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n").encode(
        "ascii"
    )
    if len(encoded) > MAX_JOURNAL_BYTES:
        raise SpikeFailure("journal-oversized")
    descriptor = os.open(
        path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC | os.O_CLOEXEC, FILE_MODE
    )
    try:
        os.fchmod(descriptor, FILE_MODE)
        os.write(descriptor, encoded)
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def read_private(path: Path) -> dict[str, Any]:
    descriptor = os.open(path, os.O_RDONLY | os.O_CLOEXEC)
    try:
        encoded = os.read(descriptor, MAX_JOURNAL_BYTES + 1)
    finally:
        os.close(descriptor)
    if len(encoded) > MAX_JOURNAL_BYTES:
        raise JournalRefused("journal-oversized")
    try:
        value = json.loads(encoded)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise JournalRefused("journal-malformed") from error
    if not isinstance(value, dict):
        raise JournalRefused("journal-malformed")
    return value


def make_handler(state: EndpointState) -> type[BaseHTTPRequestHandler]:
    class Handler(BaseHTTPRequestHandler):
        def do_POST(self) -> None:
            if self.path != "/session":
                self.send_error(404)
                return
            state.post_count += 1
            length = int(self.headers.get("Content-Length", "0"))
            self.rfile.read(min(length, MAX_JOURNAL_BYTES + 1))
            if state.mode == "ambiguous":
                self.close_connection = True
                return
            response = b'{"id":"synthetic-session"}'
            self.send_response(201)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(response)))
            self.end_headers()
            self.wfile.write(response)

        def log_message(self, *_: object) -> None:
            return

    return Handler


class FakeOpenCodeEndpoint:
    def __init__(self, mode: str) -> None:
        self.state = EndpointState(mode=mode)
        self.server = ThreadingHTTPServer(("127.0.0.1", 0), make_handler(self.state))
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)

    def __enter__(self) -> Self:
        self.thread.start()
        return self

    def __exit__(self, *_: object) -> None:
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=HTTP_TIMEOUT_SECONDS)

    def post_blank_session(self) -> str | None:
        port = self.server.server_address[1]
        connection = http.client.HTTPConnection(
            "127.0.0.1", port, timeout=HTTP_TIMEOUT_SECONDS
        )
        try:
            connection.request(
                "POST",
                "/session",
                body=b"{}",
                headers={"Content-Type": "application/json"},
            )
            response = connection.getresponse()
            body = response.read(MAX_JOURNAL_BYTES + 1)
        except (ConnectionError, OSError, http.client.HTTPException):
            return None
        finally:
            connection.close()
        if response.status != 201 or len(body) > MAX_JOURNAL_BYTES:
            return None
        try:
            value = json.loads(body)
        except (UnicodeDecodeError, json.JSONDecodeError):
            return None
        if value != {"id": "synthetic-session"}:
            return None
        return "known-binding"


class Journal:
    def __init__(self, root: Path, provider: str, *, label: str | None = None) -> None:
        self.path = root / f"{label or provider}.json"
        self.provider = provider
        self.runtime_id = uuid.uuid4().hex
        self.request_id = uuid.uuid4().hex
        self.write("issued", post_outcome="none")

    def read(self) -> dict[str, Any]:
        value = read_private(self.path)
        if (
            value.get("provider") != self.provider
            or value.get("runtime_id") != self.runtime_id
            or value.get("request_id") != self.request_id
            or value.get("phase")
            not in {
                "issued",
                "runtime_owned_launching",
                "codex_ready",
                "opencode_post_attempted",
                "opencode_binding_known",
                "provider_exec_started",
                "provider_exec_proven",
                "rolled_back_no_effect",
                "stopped_binding_recovery",
                "recovery_required",
            }
            or value.get("post_outcome") not in {"none", "known", "unknown"}
        ):
            raise JournalRefused("journal-mismatched")
        return value

    def write(self, phase: str, *, post_outcome: str) -> None:
        write_private(
            self.path,
            {
                "phase": phase,
                "post_outcome": post_outcome,
                "provider": self.provider,
                "request_id": self.request_id,
                "runtime_id": self.runtime_id,
            },
        )

    def promote(self) -> None:
        if self.read()["phase"] != "issued":
            raise JournalRefused("capability-unavailable")
        self.write("runtime_owned_launching", post_outcome="none")

    def action_allowed(self) -> bool:
        return self.read()["phase"] == "provider_exec_proven"

    def codex_prepare(self) -> None:
        if (
            self.provider != "codex"
            or self.read()["phase"] != "runtime_owned_launching"
        ):
            raise JournalRefused("codex-prepare-unavailable")
        self.write("codex_ready", post_outcome="none")

    def opencode_prepare(self, endpoint: FakeOpenCodeEndpoint) -> None:
        if (
            self.provider != "opencode"
            or self.read()["phase"] != "runtime_owned_launching"
        ):
            raise JournalRefused("opencode-prepare-unavailable")
        # Persist the attempt before the non-idempotent POST. An interrupted
        # caller therefore cannot decide that no request was sent.
        self.write("opencode_post_attempted", post_outcome="unknown")
        binding = endpoint.post_blank_session()
        if binding is None:
            self.write("recovery_required", post_outcome="unknown")
            return
        self.write("opencode_binding_known", post_outcome="known")

    def begin_exec(self) -> None:
        phase = self.read()["phase"]
        expected = (
            "codex_ready" if self.provider == "codex" else "opencode_binding_known"
        )
        if phase != expected:
            raise JournalRefused("provider-exec-unavailable")
        self.write("provider_exec_started", post_outcome=self.read()["post_outcome"])

    def prove_exec(self) -> None:
        if self.read()["phase"] != "provider_exec_started":
            raise JournalRefused("provider-exec-proof-unavailable")
        self.write("provider_exec_proven", post_outcome=self.read()["post_outcome"])

    def known_absent_exec_error(self) -> None:
        phase = self.read()["phase"]
        if phase != "provider_exec_started":
            raise JournalRefused("exec-error-unavailable")
        if self.provider == "codex":
            self.write("rolled_back_no_effect", post_outcome="none")
        else:
            # A known blank-session binding preserves the exact runtime and
            # recovery path; it must never become a clean retry.
            self.write("stopped_binding_recovery", post_outcome="known")

    def passive_recover(self) -> None:
        value = self.read()
        if value["phase"] == "runtime_owned_launching":
            self.write("rolled_back_no_effect", post_outcome="none")
        elif value["phase"] == "recovery_required":
            # This is intentionally a no-op: uncertainty is durable evidence.
            return
        else:
            raise JournalRefused("passive-recovery-unavailable")


def assert_refused(action: Any, reason: str) -> bool:
    try:
        action()
    except JournalRefused as error:
        if str(error) != reason:
            raise SpikeFailure("unexpected-refusal") from error
        return True
    raise SpikeFailure("unsafe-action-accepted")


def run_probe() -> dict[str, object]:
    root = Path(tempfile.mkdtemp(prefix=ROOT_PREFIX))
    root.chmod(ROOT_MODE)
    assertions: dict[str, bool] = {}
    try:
        codex = Journal(root, "codex")
        assertions[
            "runtime_owned_is_fenced_before_provider_effect"
        ] = not codex.action_allowed()
        codex.promote()
        assertions[
            "runtime_owned_launching_is_still_fenced"
        ] = not codex.action_allowed()
        codex.codex_prepare()
        codex.begin_exec()
        assertions["exec_started_is_still_fenced"] = not codex.action_allowed()
        codex.known_absent_exec_error()
        assertions["codex_known_absent_exec_rolls_back_only_no_effect"] = (
            codex.read()["phase"] == "rolled_back_no_effect"
            and not codex.action_allowed()
        )

        codex_crash = Journal(root, "codex", label="codex-crash")
        codex_crash.promote()
        codex_crash.passive_recover()
        assertions["pre_effect_helper_crash_can_passively_roll_back"] = (
            codex_crash.read()["phase"] == "rolled_back_no_effect"
        )

        with FakeOpenCodeEndpoint("known") as endpoint:
            known = Journal(root, "opencode")
            known.promote()
            known.opencode_prepare(endpoint)
            assertions["opencode_post_is_journaled_before_effect"] = (
                known.read()["phase"] == "opencode_binding_known"
                and known.read()["post_outcome"] == "known"
                and endpoint.state.post_count == 1
            )
            known.begin_exec()
            known.known_absent_exec_error()
            assertions["known_binding_exec_error_preserves_recovery_runtime"] = (
                known.read()["phase"] == "stopped_binding_recovery"
                and known.read()["post_outcome"] == "known"
                and endpoint.state.post_count == 1
            )
            assertions["known_binding_recovery_never_posts_again"] = (
                assert_refused(
                    lambda: known.opencode_prepare(endpoint),
                    "opencode-prepare-unavailable",
                )
                and endpoint.state.post_count == 1
            )

        with FakeOpenCodeEndpoint("ambiguous") as endpoint:
            ambiguous = Journal(root, "opencode", label="opencode-ambiguous")
            ambiguous.promote()
            ambiguous.opencode_prepare(endpoint)
            assertions["ambiguous_post_stays_recovery_required"] = (
                ambiguous.read()["phase"] == "recovery_required"
                and ambiguous.read()["post_outcome"] == "unknown"
                and endpoint.state.post_count == 1
            )
            ambiguous.passive_recover()
            assertions["ambiguous_post_recovery_never_posts_again"] = (
                ambiguous.read()["phase"] == "recovery_required"
                and assert_refused(
                    lambda: ambiguous.opencode_prepare(endpoint),
                    "opencode-prepare-unavailable",
                )
                and endpoint.state.post_count == 1
            )

        proven = Journal(root, "codex", label="codex-proven")
        proven.promote()
        proven.codex_prepare()
        proven.begin_exec()
        proven.prove_exec()
        assertions["only_exec_proof_releases_action_fence"] = proven.action_allowed()
        assertions["duplicate_or_stale_effect_transitions_refuse"] = all(
            (
                assert_refused(proven.prove_exec, "provider-exec-proof-unavailable"),
                assert_refused(codex.promote, "capability-unavailable"),
            )
        )
    finally:
        shutil.rmtree(root, ignore_errors=True)

    assertions["temporary_root_removed"] = not root.exists()
    assertions["all_case_assertions_pass"] = all(assertions.values())
    return {
        "contract": CONTRACT,
        "status": "pass" if assertions["all_case_assertions_pass"] else "falsified",
        "reason": "provider-effect-journal-ordering-observed",
        "assertions": assertions,
    }


def parse_arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--result", type=Path, required=True)
    return parser.parse_args()


def main() -> int:
    arguments = parse_arguments()
    result = run_probe()
    path = arguments.result
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        json.dumps(result, sort_keys=True, indent=2) + "\n", encoding="utf-8"
    )
    path.chmod(FILE_MODE)
    return 0 if result["status"] == "pass" else 1


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (JournalRefused, SpikeFailure) as error:
        print(f"{STUDY}: {error}", file=os.sys.stderr)
        raise SystemExit(1) from error
