#!/usr/bin/env bash
set -euo pipefail

# Static/source acceptance for the active schema-14 D17 control plane. Every
# CLI rejection runs against a disposable empty directory; no ordinary state
# or provider process is opened.
workspace_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
cd "${workspace_root}"

cargo build --locked --bin wsnav >/dev/null
wsnav_bin="${workspace_root}/target/debug/wsnav"

retired_commands=(
    setup trust-observer update-observer register-remote host
    _remote _probe _attach _navigator _presentation_shell _provider_attach
    _observer_review _presentation_ssh_shell _provider_remote_attach
)
for retired in "${retired_commands[@]}"; do
    state_root="$(mktemp -d "${TMPDIR:-/tmp}/wsnav-d17-rejected.XXXXXX")"
    trap 'rm -rf -- "${state_root}"' EXIT
    if "${wsnav_bin}" --state-root "${state_root}" "${retired}" \
        >"${state_root}/stdout" 2>"${state_root}/stderr"; then
        printf 'error: retired command unexpectedly succeeded: %s\n' "${retired}" >&2
        exit 1
    fi
    # Clap rejects before StateRoot selection; only the test captures may
    # exist, never a database, lease, marker, or presentation directory.
    if find "${state_root}" -mindepth 1 -maxdepth 1 \
        ! -name stdout ! -name stderr -print -quit | grep -q .; then
        printf 'error: retired command created state: %s\n' "${retired}" >&2
        exit 1
    fi
    rm -rf -- "${state_root}"
    trap - EXIT
done

retired_files=(
    src/remote.rs src/transport.rs src/build_info.rs src/protocol/mod.rs
    src/app/remote.rs src/app/lifecycle.rs src/application.rs
    src/navigator/d16.rs src/navigator/d16_controller.rs
)
for retired_file in "${retired_files[@]}"; do
    if [[ -e "${retired_file}" ]]; then
        printf 'error: retired source file remains: %s\n' "${retired_file}" >&2
        exit 1
    fi
done

retired_symbols=(
    'HostRegistry::open' ClientCatalog RemoteExecutable SshDestination SshEndpoint
    CommandRunner execute_state_command RegisterRemote HostCommands LocalApplication
    D16Navigator run_local_navigator create_or_focus_shell
)
for retired_symbol in "${retired_symbols[@]}"; do
    if rg -n --fixed-strings "${retired_symbol}" src --glob '*.rs' >/dev/null; then
        printf 'error: retired source symbol remains: %s\n' "${retired_symbol}" >&2
        exit 1
    fi
done

# The three removed hidden commands may remain only as inert argv evidence in
# the legacy presentation classifier. They must never re-enter CLI, dispatch,
# Navigator, provider-launch, or provisional-onboarding code.
active_route_sources=(src/app.rs src/app src/navigator.rs src/navigator src/provisional.rs)
for retired_route in _navigator _presentation_shell _provider_attach; do
    route_literal="\"${retired_route}\""
    if rg -n --fixed-strings "${route_literal}" "${active_route_sources[@]}" \
        --glob '*.rs' --glob '!tests.rs' >/dev/null; then
        printf 'error: retired hidden route remains active: %s\n' "${retired_route}" >&2
        exit 1
    fi
done

# Production control-plane code uses bounded process helpers; direct
# Command::output would permit unbounded output and is forbidden here.
production_sources=(
    src/app.rs src/app src/actions src/cutover.rs src/navigator src/presentation.rs
    src/provisional.rs src/provider src/repository.rs src/startup.rs src/state
)
if rg -n '\.output\(' "${production_sources[@]}" --glob '*.rs' >/dev/null; then
    printf 'error: direct Command::output remains in production control-plane source\n' >&2
    exit 1
fi

# D17 is the active host-local product boundary. Active implementation files
# must not drift back into staged/inactive commentary that obscures authority.
active_d17_sources=(
    src/app/dispatch.rs src/navigator/d17.rs src/navigator/d17_controller.rs
    src/presentation.rs src/provisional.rs src/state/d16.rs
)
if rg -n -i '\binactive\b|future[- ](?:checkpoint|only)|\bstaged boundary\b' \
    "${active_d17_sources[@]}" >/dev/null; then
    printf 'error: active D17 source still describes itself as staged or inactive\n' >&2
    exit 1
fi

printf 'D17 source/CLI acceptance passed\n'
