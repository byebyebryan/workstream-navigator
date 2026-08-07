#!/usr/bin/env bash
# Deterministic D8.2 production-harness regression checks.  This validates
# sanitized cleanup evidence only; it never invokes OpenCode or WSNav.
set -euo pipefail

workspace_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
python3 -B - "${workspace_root}" <<'PY'
import importlib.util
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path

workspace = Path(sys.argv[1]).resolve()
spikes = workspace / "spikes"
sys.path.insert(0, str(spikes))
source = spikes / "opencode-production-d8.2.py"
spec = importlib.util.spec_from_file_location("opencode_production_d82", source)
if spec is None or spec.loader is None:
    raise SystemExit("unable to load D8.2 harness")
harness = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = harness
spec.loader.exec_module(harness)
read_identity = harness.read_process_identity

reference = harness.RootReference(
    pid=42,
    birth="123456",
    process_group=42,
    session=41,
    category="environment",
)
rendered = harness.format_root_reference(reference)
assert rendered == (
    "cleanup-root-reference-present:environment:pid=42:birth=123456:"
    "pgrp=42:session=41"
)
assert "provider" not in rendered
assert str(reference) not in rendered

assert harness.bounded_cleanup_reason(OSError("/secret/provider-payload")) == (
    "cleanup-error:OSError"
)
diagnostics = []
harness.record_cleanup_diagnostic(diagnostics, rendered)
harness.record_cleanup_diagnostic(diagnostics, rendered)
assert diagnostics == [rendered]
assert harness.compose_cleanup_reason("falsified:fork-boundary", diagnostics).startswith(
    "falsified:fork-boundary;cleanup-incomplete="
)
assert len(harness.compose_cleanup_reason("primary", ["x" * 192] * 8)) <= 512
harness.read_process_identity = lambda _pid: harness.ProcessIdentity(
    123, "2", 1, 1, 1, "R"
)
ambiguous = harness._root_reference(
    Path("123"), "cwd", harness.ProcessIdentity(123, "1", 1, 1, 1, "R")
)
assert ambiguous.category == "identity-ambiguous"
assert ambiguous.birth is None
harness.read_process_identity = read_identity

failure = subprocess.CompletedProcess(
    ["wsnav"],
    1,
    stdout="private provider output",
    stderr="error: host rejected the request: workstream creation is unavailable",
)
assert harness.bounded_wsnav_failure(failure, "host-fork") == (
    "wsnav-command-failed:host-fork:creation-unavailable"
)
failure.stderr = "/secret/provider-payload"
assert harness.bounded_wsnav_failure(failure, "host-fork") == (
    "wsnav-command-failed:host-fork:other"
)

calls = []
harness.runtime_info = lambda _state, _workstream: {
    "runtime_id": "runtime",
    "runtime_lifecycle": "stopped",
}
harness.capture_provider_evidence = lambda _state, _runtime: None
harness.cleanup_provider_group = lambda *args, **kwargs: calls.append(kwargs)
harness.private_socket = lambda state, _runtime: state / "missing.sock"
harness.park_direct(
    Path("/tmp/wsnav-d82-state"),
    Path("/tmp/wsnav-d82-state"),
    "workstream",
    {},
    reference_root=Path("/tmp/wsnav-d82-root"),
    check_root=False,
)
assert calls == [{"check_root": False}]

cleanup_attempts = []


def transient_cleanup():
    cleanup_attempts.append(True)
    if len(cleanup_attempts) == 1:
        raise harness.AcceptanceFailure("cleanup-runtime-transitioning")


harness.retry_cleanup_action(transient_cleanup, timeout=0.1, poll_interval=0.001)
assert len(cleanup_attempts) == 2

with tempfile.TemporaryDirectory(prefix="wsnav-d82-harness.") as root_text:
    root = Path(root_text)
    # The cleanup authority itself may keep a root-owned descriptor open;
    # only an external process should be reported as a root culprit.
    with (root / "self-owned").open("wb") as self_owned:
        self_owned.write(b"disposable\n")
        self_owned.flush()
        assert harness.process_references_root(root) is None
        process = subprocess.Popen(["sleep", "10"], cwd=root)
        try:
            found = harness.process_references_root(root)
            assert found is not None
            assert found.category == "cwd"
            assert found.pid == process.pid
            assert found.birth is not None and found.birth.isdecimal()
            assert root_text not in harness.format_root_reference(found)
        finally:
            process.terminate()
            process.wait(timeout=5)

    removed = root / "removed"

    def recreate_root():
        time.sleep(0.05)
        removed.mkdir()

    recreator = threading.Thread(target=recreate_root)
    recreator.start()
    recreated = harness.wait_for_root_removed(removed, timeout=1)
    recreator.join(timeout=1)
    assert recreated is not None
    assert recreated.category == "root-remains"

print("D8.2 production-harness diagnostics checks passed")
PY
