#!/usr/bin/env bash
# Deterministic non-live D12 harness checks. This wrapper never starts a
# provider, SSH daemon, tmux server, or ordinary-state mutation.
set -euo pipefail

workspace_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
python3 -B "${workspace_root}/spikes/d12-presentation-acceptance.py" --self-test

result_file="$(mktemp)"
trap 'rm -f -- "${result_file}"' EXIT
if python3 -B "${workspace_root}/spikes/d12-presentation-acceptance.py" \
    --result "${result_file}"; then
    printf 'default D12 harness unexpectedly passed\n' >&2
    exit 1
fi
python3 -B - "${result_file}" <<'PY'
import json
import stat
import sys
from pathlib import Path

result_path = Path(sys.argv[1])
assert stat.S_IMODE(result_path.stat().st_mode) == 0o600
encoded = result_path.read_text(encoding="utf-8")
result = json.loads(encoded)
assert result["status"] == "blocked"
assert result["reason"] == "operator-confirmation-required"
assert result["primary_status"] == "blocked"
assert result["primary_reason"] == "operator-confirmation-required"
assert result["operator_confirmed"] is False
assert all(value is False for value in result["assertions"].values())
assert "/" not in encoded
PY

printf 'd12 presentation harness non-live checks passed\n'
