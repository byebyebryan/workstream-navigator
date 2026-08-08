#!/usr/bin/env bash
# Deterministic D8.2 production-harness regression checks.  This validates
# sanitized cleanup evidence only; it never invokes OpenCode or WSNav.
set -euo pipefail

workspace_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
python3 -B - "${workspace_root}" <<'PY'
import importlib.util
import sqlite3
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


def source(
    revision,
    *,
    settled_id="turn-1",
    runtime_id="runtime-1",
    session_id="session-1",
    generation="generation-1",
):
    return {
        "revision": revision,
        "settled_id": settled_id,
        "provider": "opencode",
        "runtime_id": runtime_id,
        "session_id": session_id,
        "handle_generation": generation,
        "tmux_generation": "tmux-generation-1",
    }


class FakeClock:
    def __init__(self):
        self.value = 0.0

    def now(self):
        return self.value

    def sleep(self, duration):
        self.value += duration


def scripted_reader(values):
    calls = []

    def read(_state, _workstream):
        calls.append(True)
        return values[min(len(calls) - 1, len(values) - 1)]

    return read, calls


baseline = source(1)
clock = FakeClock()
reader, reads = scripted_reader([source(1), source(2), source(3), source(3), source(3)])
stable = harness.wait_for_stable_settled_source(
    Path("/tmp/wsnav-d82-state"),
    "source",
    baseline=baseline,
    timeout=1,
    stable_window=0.2,
    poll_interval=0.1,
    read=reader,
    clock=clock.now,
    sleep=clock.sleep,
)
assert stable["revision"] == 3
assert len(reads) == 5

clock = FakeClock()
revision = [0]


def continuously_changing(_state, _workstream):
    revision[0] += 1
    return source(revision[0])


try:
    harness.wait_for_stable_settled_source(
        Path("/tmp/wsnav-d82-state"),
        "source",
        baseline=baseline,
        timeout=0.31,
        stable_window=0.2,
        poll_interval=0.1,
        read=continuously_changing,
        clock=clock.now,
        sleep=clock.sleep,
    )
except harness.AcceptanceFailure as error:
    assert str(error) == "observer-revision-churn"
else:
    raise AssertionError("continuous observer churn unexpectedly stabilized")
assert revision[0] <= 5

fork_calls = []
effect_checks = []
responses = [
    subprocess.CompletedProcess(
        ["wsnav"],
        1,
        stdout="",
        stderr="error: host rejected the request: revision conflict; refresh this host",
    ),
    subprocess.CompletedProcess(["wsnav"], 0, stdout="created ws-2\n", stderr=""),
]
forked = harness.invoke_fork_with_revision_retry(
    "source",
    baseline,
    invoke=lambda workstream, revision: (
        fork_calls.append((workstream, revision)) or responses.pop(0)
    ),
    refresh=lambda _source: source(2),
    assert_no_effect=lambda: effect_checks.append(True),
)
assert forked.returncode == 0
assert fork_calls == [("source", "1"), ("source", "2")]
assert effect_checks == [True]

second_conflict_calls = []
second_conflict = subprocess.CompletedProcess(
    ["wsnav"],
    1,
    stdout="",
    stderr="error: host rejected the request: revision conflict; refresh this host",
)
try:
    harness.invoke_fork_with_revision_retry(
        "source",
        baseline,
        invoke=lambda workstream, revision: (
            second_conflict_calls.append((workstream, revision)) or second_conflict
        ),
        refresh=lambda _source: source(2),
        assert_no_effect=lambda: None,
    )
except harness.AcceptanceFailure as error:
    assert str(error) == "observer-revision-churn"
else:
    raise AssertionError("second revision conflict unexpectedly succeeded")
assert second_conflict_calls == [("source", "1"), ("source", "2")]

effect_evidence_calls = []
try:
    harness.invoke_fork_with_revision_retry(
        "source",
        baseline,
        invoke=lambda workstream, revision: (
            effect_evidence_calls.append((workstream, revision)) or second_conflict
        ),
        refresh=lambda _source: (_ for _ in ()).throw(
            AssertionError("effect evidence failure was retried")
        ),
        assert_no_effect=lambda: (_ for _ in ()).throw(
            harness.AcceptanceFailure("fork-effect-observed")
        ),
    )
except harness.AcceptanceFailure as error:
    assert str(error) == "fork-effect-observed"
else:
    raise AssertionError("Fork effect evidence unexpectedly passed")
assert effect_evidence_calls == [("source", "1")]

nonrevision_calls = []
nonrevision = subprocess.CompletedProcess(
    ["wsnav"],
    1,
    stdout="",
    stderr="error: workstream creation is unavailable; /secret/provider-payload",
)
try:
    harness.invoke_fork_with_revision_retry(
        "source",
        baseline,
        invoke=lambda workstream, revision: (
            nonrevision_calls.append((workstream, revision)) or nonrevision
        ),
        refresh=lambda _source: (_ for _ in ()).throw(
            AssertionError("non-revision failure was retried")
        ),
        assert_no_effect=lambda: (_ for _ in ()).throw(
            AssertionError("non-revision failure requested effect evidence")
        ),
    )
except harness.AcceptanceFailure as error:
    assert str(error) == "wsnav-command-failed:host-fork:creation-unavailable"
else:
    raise AssertionError("non-revision failure unexpectedly succeeded")
assert nonrevision_calls == [("source", "1")]
assert not harness.is_pre_effect_revision_conflict(nonrevision)

exact_conflict = subprocess.CompletedProcess(
    ["wsnav"],
    1,
    stdout="",
    stderr="error: host rejected the request: revision conflict; refresh this host",
)
assert harness.is_pre_effect_revision_conflict(exact_conflict)
for spoofed in (
    "prefix error: host rejected the request: revision conflict; refresh this host",
    "error: host rejected the request: revision conflict; refresh this host suffix",
    "revision conflict; refresh this host",
):
    spoof = subprocess.CompletedProcess(
        ["wsnav"], 1, stdout="", stderr=spoofed
    )
    assert not harness.is_pre_effect_revision_conflict(spoof)
    calls = []
    try:
        harness.invoke_fork_with_revision_retry(
            "source",
            baseline,
            invoke=lambda workstream, revision: (
                calls.append((workstream, revision)) or spoof
            ),
            refresh=lambda _source: (_ for _ in ()).throw(
                AssertionError("spoofed conflict was retried")
            ),
            assert_no_effect=lambda: (_ for _ in ()).throw(
                AssertionError("spoofed conflict requested effect evidence")
            ),
        )
    except harness.AcceptanceFailure:
        pass
    else:
        raise AssertionError("spoofed conflict unexpectedly succeeded")
    assert calls == [("source", "1")]

for missing in (
    "provider",
    "runtime_id",
    "session_id",
    "handle_generation",
    "tmux_generation",
):
    incomplete = source(1)
    incomplete.pop(missing)
    assert harness._settled_source_sample(incomplete) is None
not_opencode = source(1)
not_opencode["provider"] = "codex"
assert harness._settled_source_sample(not_opencode) is None
assert harness._settled_source_sample(source(True)) is None
assert harness._settled_source_sample(source(1.5)) is None
assert harness._settled_source_sample(source("1")) is None

regression_reader, _ = scripted_reader([source(1)])
try:
    harness.wait_for_stable_settled_source(
        Path("/tmp/wsnav-d82-state"),
        "source",
        baseline=source(2),
        timeout=1,
        read=regression_reader,
        clock=lambda: 0.0,
        sleep=lambda _duration: None,
    )
except harness.AcceptanceFailure as error:
    assert str(error) == "observer-revision-regressed"
else:
    raise AssertionError("revision regression unexpectedly accepted")

boundary_reader, _ = scripted_reader([source(2, settled_id="turn-2")])
try:
    harness.wait_for_stable_settled_source(
        Path("/tmp/wsnav-d82-state"),
        "source",
        baseline=baseline,
        timeout=1,
        read=boundary_reader,
        clock=lambda: 0.0,
        sleep=lambda _duration: None,
    )
except harness.AcceptanceFailure as error:
    assert str(error) == "observer-settled-boundary-changed"
else:
    raise AssertionError("settled boundary change unexpectedly accepted")

whitespace_boundary_reader, _ = scripted_reader([source(2, settled_id=" turn-1")])
try:
    harness.wait_for_stable_settled_source(
        Path("/tmp/wsnav-d82-state"),
        "source",
        baseline=baseline,
        timeout=1,
        read=whitespace_boundary_reader,
        clock=lambda: 0.0,
        sleep=lambda _duration: None,
    )
except harness.AcceptanceFailure as error:
    assert str(error) == "observer-settled-boundary-changed"
else:
    raise AssertionError("normalized settled boundary unexpectedly accepted")

whitespace_runtime_reader, _ = scripted_reader([source(2, runtime_id=" runtime-1")])
try:
    harness.wait_for_stable_settled_source(
        Path("/tmp/wsnav-d82-state"),
        "source",
        baseline=baseline,
        timeout=1,
        read=whitespace_runtime_reader,
        clock=lambda: 0.0,
        sleep=lambda _duration: None,
    )
except harness.AcceptanceFailure as error:
    assert str(error) == "observer-runtime-changed"
else:
    raise AssertionError("normalized Runtime identity unexpectedly accepted")

with tempfile.TemporaryDirectory(prefix="wsnav-d82-fork-effect.") as effect_root:
    effect_state = Path(effect_root)
    with sqlite3.connect(effect_state / "host.sqlite") as connection:
        connection.executescript(
            """
            CREATE TABLE compound_operations (operation_id TEXT PRIMARY KEY);
            CREATE TABLE workstreams (
                workstream_id TEXT PRIMARY KEY,
                source_workstream_id TEXT
            );
            INSERT INTO compound_operations VALUES ('existing-operation');
            INSERT INTO workstreams VALUES ('source', NULL);
            """
        )
    effect_baseline = harness.fork_effect_baseline(effect_state, "source")
    harness.assert_fork_effect_unchanged(effect_state, "source", effect_baseline)
    with sqlite3.connect(effect_state / "host.sqlite") as connection:
        connection.execute(
            "INSERT INTO workstreams VALUES ('destination', 'source')"
        )
    try:
        harness.assert_fork_effect_unchanged(effect_state, "source", effect_baseline)
    except harness.AcceptanceFailure as error:
        assert str(error) == "fork-effect-observed"
    else:
        raise AssertionError("durable Fork effect unexpectedly passed retry gate")

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
