#!/usr/bin/env bash
set -euo pipefail

# Static and CLI acceptance for the current schema-15 product. Help and parser
# checks do not select a state root, launch a provider, or contact tmux.
workspace_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
cd "${workspace_root}"

cargo build --locked --bin wsnav >/dev/null
wsnav_bin="${workspace_root}/target/debug/wsnav"
help_text="$("${wsnav_bin}" --help)"

public_commands=(
    navigator doctor remove-observer fork-workstream start recover attach park
    archive restore status operations recover-operation acknowledge
)
for command in "${public_commands[@]}"; do
    if ! rg -q "^  ${command}( |$)" <<<"${help_text}"; then
        printf 'error: public command missing from generated help: %s\n' "${command}" >&2
        exit 1
    fi
done

retired_commands=(
    setup trust-observer update-observer register-remote host register
    new-workstream rename navigator_d17 _remote _probe _attach _presentation_shell
    _observer_review _presentation_ssh_shell _provider_remote_attach
)
for command in "${retired_commands[@]}"; do
    if rg -q "^  ${command}( |$)" <<<"${help_text}"; then
        printf 'error: retired command remains in generated help: %s\n' "${command}" >&2
        exit 1
    fi
done

retired_files=(
    src/cutover.rs src/d17_account_shell.rs src/d17_broker.rs src/d17_clock.rs
    src/d17_helper.rs src/d17_reconcile.rs src/d17_review.rs
    src/d17_shell_control.rs src/d17_shell_gate.rs src/d17_snapshot.rs
    src/navigator/d16.rs src/navigator/d16_controller.rs src/navigator/d17.rs
    src/navigator/d17_controller.rs src/presentation.rs src/provider/d17_grammar.rs
    src/state/d16.rs
)
for path in "${retired_files[@]}"; do
    if [[ -e "${path}" ]]; then
        printf 'error: retired source path remains: %s\n' "${path}" >&2
        exit 1
    fi
done

current_modules=(
    src/navigator/controller.rs src/navigator/view.rs src/provider/lifecycle.rs
    src/provider/grammar.rs src/state/current/bootstrap.rs
    src/state/current/registry.rs src/state/current/onboarding.rs
    src/state/current/observer.rs src/state/current/projection.rs
    src/state/current/schema.rs src/presentation/ownership.rs
    src/presentation/topology.rs src/presentation/control.rs
    src/presentation/attachment.rs src/presentation/provisional.rs
    src/presentation/cleanup.rs
)
for path in "${current_modules[@]}"; do
    if [[ ! -f "${path}" ]]; then
        printf 'error: semantic current module missing: %s\n' "${path}" >&2
        exit 1
    fi
done

# Delivery-checkpoint names are allowed only as exact negative evidence proving
# an old hidden route and durable table name are absent.
if rg -n -i 'd16|d17' src tests --glob '*.rs' \
    --glob '!src/app/tests.rs' --glob '!src/state/current_state_tests.rs' >/dev/null; then
    printf 'error: checkpoint-qualified operational identifier remains active\n' >&2
    exit 1
fi
if [[ "$(rg -n -i 'd16|d17' src/app/tests.rs --glob '*.rs' | wc -l)" -ne 1 ]] \
    || ! rg -n --fixed-strings 'navigator_d17' src/app/tests.rs >/dev/null; then
    printf 'error: old CLI negative-evidence allowlist changed\n' >&2
    exit 1
fi
if [[ "$(rg -n -i 'd16|d17' src/state/current_state_tests.rs --glob '*.rs' | wc -l)" -ne 1 ]] \
    || ! rg -n --fixed-strings 'd17_onboarding_exec_targets' \
        src/state/current_state_tests.rs >/dev/null; then
    printf 'error: old table negative-evidence allowlist changed\n' >&2
    exit 1
fi

if rg -n '^//!  |#\[error\(" |ControlRefused\(" |unreachable!\(" ' \
    src --glob '*.rs' >/dev/null; then
    printf 'error: malformed current-source diagnostic or module documentation remains\n' >&2
    exit 1
fi

if rg -n '\.output\(' src --glob '*.rs' >/dev/null; then
    printf 'error: direct Command::output remains in production source\n' >&2
    exit 1
fi

printf 'current source and CLI acceptance passed\n'
