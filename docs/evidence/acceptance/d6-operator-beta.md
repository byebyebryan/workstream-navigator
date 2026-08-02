# D6 Source-Installed Operator-Beta Acceptance

Date: 2026-07-31

Status: pass — the exact candidate completed the repository, release-parity,
local-presentation, and SSH-native smoke gates without widening V1.

Historical evidence note: this file records the D6 candidate. Its candidate
hash, protocol/schema values, and test counts are not the current runtime
fingerprint; use the [design](../../design.md), [roadmap](../../roadmap.md), and
`wsnav host doctor <alias>` for the current contract and installed-build
compatibility.

## Evidence

- The README, design, and roadmap describe the implemented D0-D6 product in
  present tense. V1 is explicitly a source-installed `0.1.0` operator beta;
  there is no tag, hosted binary, updater, automatic remote deployment, or
  Cargo publication.
- Local and registered-SSH release binaries were built from candidate
  `8692695`. Their binary hashes, package version, control ABI, protocol, and
  host-schema fingerprints matched. The stateless remote doctor reported
  release compatibility ready before the stateful smoke.
- Converting an active development-symlink installation to a stable executable
  path correctly failed the exact observer-ownership check. Existing managed
  Runtimes prevented an unsafe profile update, the original development
  installation was restored exactly, and both observer doctors returned
  `Ready`. The documented migration now requires parking every managed Runtime,
  updating the observer declaration, and completing native trust review again.
- The operator smoke opened the current local two-pane presentation and reused
  one retained parked SSH Workstream. Native resume attached directly in the
  provider pane, one deterministic no-file-change turn completed, and the
  navigator showed result attention without writing management text into the
  provider pane.
- A single mouse click moved focus from the provider pane to blank navigator
  space and another single click moved it back. The Workstream parked, reopened
  with its completed native result still visible, and parked again. No new
  Workstream, Checkout, provider conversation, or unresolved operation was
  created.
- The first smoke exposed an outer-client race: `q` stopped the private
  presentation but the parent reported a failed tmux attach. Candidate
  `8692695` now distinguishes an expected stopped owned presentation from a
  failed attach to a still-live presentation. Focused tests cover both
  classifications, and the reproduced native quit exits zero while removing
  its ephemeral presentation directory.
- Default-tmux fingerprints on both hosts were identical before and after. One
  pre-existing remote managed Runtime retained its exact Runtime and process
  birth identity throughout the smoke. The reused acceptance Workstream ended
  parked, both observer profiles remained ready, and both source checkouts
  remained clean.
- Exact-candidate GitHub CI passed both the current stable job and the declared
  Rust 1.88 job. `scripts/check` passes 157 library tests and three integration
  tests, formatting, Clippy, package verification, Cargo Deny, script/fixture
  checks, every disposable D4-D5.1 acceptance harness, and `git diff --check`.

The [sanitized D6 fixture](../../../spikes/fixtures/d6-operator-beta.json) contains
only fixed capability, isolation, cleanup, distribution, and privacy
assertions. It contains no provider or Workstream identifiers, prompts,
results, terminal captures, paths, process IDs, credentials, or raw payloads.
