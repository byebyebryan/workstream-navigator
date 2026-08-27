# Spike 0024: D17 provider fresh-TUI grammar

## Question

Can D17 pin a small, version-specific command grammar for the installed Codex
and OpenCode CLIs that distinguishes a promotable fresh native TUI from
explicitly unmanaged information/auth commands and all broker-owned or
identity-changing shapes before a broker reservation exists?

## Procedure and isolation

The deterministic harness is
[`spikes/d17-provider-grammar.py`](../../../spikes/d17-provider-grammar.py),
with its sanitized [fixture][fixture]. It starts no provider and creates no
state, tmux server, session, or provider home. It reads only `--version` and
`--help` from the installed binaries, asserts the native surfaces expose the
expected rejected boundaries, then applies a closed parser to an in-process
matrix.

The pinned grammar admits only a no-positional fresh TUI plus a bounded,
duplicate-free set of native display/model/agent/sandbox/approval options.
Codex local provider selection requires explicit `--oss`. OpenCode project,
continue/session/fork, prompt, attach, and server options refuse. Codex prompt,
resume/fork, remote, profile, cwd, config, image, and dangerous-bypass options
refuse. Ambiguous `--option=value`, values with whitespace/control characters,
known secret-like prefixes, oversized values, and repeated options also refuse.

Exact bare help/version commands—and Codex `login` and OpenCode `providers`—are
classified as explicitly unmanaged. The fixture contains only the installed
versions, contract label, and boolean results, never raw help text or command
values.

## Result

The fixture passed against Codex `0.150.0` and OpenCode `1.18.23`.

- Each provider's installed help surface exposed the expected fresh-TUI and
  rejected identity/session/server boundaries.
- Empty invocations and the selected safe native-option matrices classified as
  `managed-fresh`; short aliases normalize to one deterministic argv digest.
- All explicit unmanaged information/auth shapes classified as unmanaged rather
  than promotion candidates.
- Every tested project/path, resume/session/fork, attach/server, profile/cwd,
  prompt, config/image, dangerous, malformed, secret-like, oversized, and
  duplicate form refused before reservation.

## Consequence

D17 may implement these exact adapter contracts for the recorded versions.
Unknown flags, version drift, or a grammar extension require a new explicit
contract and evidence; the broker must never strip or reinterpret a rejected
argument to make it launchable.

## Limits

- This is a parser/installed-help study, not the Rust broker or helper. It does
  not prove quoting, actual provider effects, session binding, Codex observer
  readiness, OpenCode `POST /session`, or recovery.
- It intentionally keeps the managed grammar conservative. A native option not
  listed here is not silently safe merely because a provider accepts it.

## Status

**Pinned fresh-TUI grammar validated for Codex 0.150.0 and OpenCode 1.18.23;
D17.0 effect and recovery integration remains required.**

[fixture]: ../../../spikes/fixtures/d17-provider-grammar.json
