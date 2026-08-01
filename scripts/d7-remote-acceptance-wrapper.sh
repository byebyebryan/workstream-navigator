#!/usr/bin/env bash
# Temporary remote D7 native-acceptance endpoint.  The operator copies this
# beside the candidate binary into one disposable directory and, for the
# duration of the test only, makes the standard remote executable path point
# here.  This keeps remote state and Codex-owned provider data out of the
# ordinary WSNav and Codex homes without weakening the fixed SSH command
# contract exercised by the navigator.
set -euo pipefail

wrapper_path="$(readlink -f "${BASH_SOURCE[0]}")"
acceptance_root="$(cd "$(dirname "$wrapper_path")" && pwd)"
export CODEX_HOME="$acceptance_root/codex-home"

exec "$acceptance_root/wsnav-bin" --state-root "$acceptance_root/state" "$@"
