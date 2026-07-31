# D6.1 Project-Identity Acceptance

Date: 2026-07-31

Status: passed

## Hypothesis

An externally created linked worktree can remain the initial Workstream
checkout while Workstream Navigator identifies its primary repository
separately. Exact credential-free canonical fetch-remote fingerprints can then
group matching ProjectLocations across hosts without sharing host authority or
exposing repository URLs and paths.

## Procedure

- Exercised pure remote normalization against SSH, SSH URL, and HTTPS spellings
  plus credentialed, local-path, missing, and conflicting remotes.
- Created a disposable non-bare repository, commit, linked worktree, nested
  registration path, and canonical fetch remote.
- Registered the nested linked-worktree path through the production CLI using
  a disposable state root.
- Verified the external Checkout, primary repository command path, bounded
  display label, and opaque fingerprint relationships in private disposable
  state.
- Exercised client catalog association for matching and different
  fingerprints on distinct host identities.
- Migrated disposable D6 host and client schemas and verified that existing
  Location associations and labels were retained or refreshed correctly.
- Ran `scripts/check`, including formatting, lint, all-target tests, package
  verification, dependency policy, disposable acceptance scripts, and Git diff
  validation.

## Observed contract

- A subdirectory below a linked worktree resolves to that linked worktree's
  root for the external Checkout.
- The primary non-bare worktree is stored separately as the ProjectLocation's
  Git command path.
- The exact selected Checkout commit remains the configured default base; no
  fetch or branch synchronization occurs.
- Transport-only URL differences normalize to one `git-remote-v1` SHA-256
  fingerprint.
- A conventional `origin` is preferred. With no `origin`, exactly one
  unambiguous fetch remote is accepted. Missing, local-path, unsupported, or
  conflicting remotes produce no fingerprint.
- Matching fingerprints reuse one client-generated Project ID and its stable
  display label. Different or absent fingerprints remain separate.
- Host actions continue to address opaque host Location and Workstream IDs;
  Project grouping grants no cross-host mutation authority.
- The protocol is version 8 and the host development schema is version 5, so
  manually installed local and SSH binaries must still match.

## Validation

- Rust tests: 168 passed across unit and local-subprocess suites.
- Clippy: all targets and features passed with warnings denied.
- Package build and verification: passed.
- Dependency advisories, bans, licenses, and sources: passed under the existing
  policy; only the policy's pre-existing informational duplicate and unused
  license-allowance warnings remained.
- Disposable D4, D5, fresh-install, and D5.1 acceptance suites: passed.
- Disposable linked-worktree CLI registration: passed with complete cleanup.
- `git diff --check`: passed.

## Isolation and privacy

The focused CLI check used only a temporary repository, linked worktree, and
state root, then removed them. It did not open the normal Workstream Navigator
state, contact an SSH host, launch Codex, or access the user's tmux server.

No raw remote URL, credential, repository path, common-directory path,
Workstream or provider identifier, process ID, prompt, result, terminal
capture, transcript, or hook payload is recorded in this document. Only
sanitized relationships, versions, assertion counts, and cleanup status are
retained.

## Limitations

- Project grouping is presentation metadata in one client catalog; separate
  navigator clients do not synchronize their generated Project IDs.
- Repository fingerprints are captured from local Git configuration without
  network verification. Mirrors or intentionally different remote spellings
  may remain separate.
- An `upstream` shared by a fork and its parent does not override distinct
  `origin` identities.
- This checkpoint did not modify or deploy the executable on a real SSH host;
  the existing matching-build doctor and manual installation requirement still
  applies.
