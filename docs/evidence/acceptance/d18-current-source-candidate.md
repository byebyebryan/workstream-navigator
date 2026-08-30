# D18 current-only consolidation acceptance

Date: 2026-08-29

Status: accepted release checkpoint `c961c7e`. Repository checks, the
authorized destructive reset, direct ordinary schema-15 bootstrap, installed
parity, an 80-column shell-first launch, Rust 1.88 and Ubuntu 24.04 clean-host
matrices, explicit native observer trust, disposable Codex/OpenCode lifecycle
acceptance, and complete cleanup passed for that artifact. Later
test/documentation traceability commit `08f9265` exposed a startup race in CI;
correction `ed0d883` and its local verification are recorded below without
transferring the historical installation or live-provider claims to the newer
source.

## Candidate boundary

- Fresh state directly creates schema 15 with application ID `0x57534e56`.
- Schemas 12 through 14 and retired transition artifacts are inert refusal
  evidence; no migration, import, adoption, or compatibility route remains.
- Current state, presentation, Navigator, onboarding, provider grammar, and
  reconciliation code use semantic modules and hidden role names.
- Projects browsing and arbitrary registration are absent. The provisional
  account shell remains the path and provider-selection surface.
- The installed binary and release build are byte-identical at SHA-256
  `f732e2b16344b038cd05996501ce77be42302f7403de9720d156dbf24777d124`.

## Repository evidence

The final local repository gate passes formatting, strict Clippy, all retained tests,
packaging, dependency policy, shell/Python/fixture checks, generated-help and
semantic-source acceptance, disposable presentation/state acceptance, and
staged/unstaged diff checks.

### Automated contract traceability

This post-acceptance index maps the current-only matrix in
[`docs/design.md`](../../design.md#d18-acceptance-contract) to representative
tests in the current candidate. It adds no live-provider acceptance claim and
does not substitute automated evidence for the separately authorized run
below. `scripts/check` executes every listed test through the locked
all-targets/all-features suite; the semantic presentation/state wrapper reruns
its current-contract subsets.

| Contract surface | Representative deterministic proof |
| --- | --- |
| Direct schema-15 creation, restart, raw old/future refusal, and effect-unknown bootstrap boundaries | `state::current_state_tests::{direct_schema15_bootstrap_publishes_current_identity_and_lock,current_open_refuses_old_header_without_sqlite_open,bootstrap_restarts_at_each_durable_phase_boundary,staging_sidecar_and_stage_final_coexistence_are_effect_unknown}` |
| Brokered Shell promotion, capability consumption, exact handoff ownership, and fresh-Shell selection | `onboarding_broker::tests::broker_reserves_and_consumes_once_after_exact_marker_shell_and_grammar_proof`, `provisional::tests::ownership_consume_removes_cleanup_authority_and_fences_actions_until_exec_proof`, and `navigator::view::tests::promotion_transfers_selection_from_shell_to_its_managed_runtime_card` |
| Exact attachment, private presentation topology, restart ownership, and native terminal input | `actions::tests::codex_attachment_requires_a_complete_recorded_process_identity`, `presentation::tests::provider_attachment_carries_exact_snapshot_revisions`, and `presentation_recovery::{navigator_stop_leaves_cleanup_to_the_outer_owner,nested_runtime_literal_ctrl_b_reaches_the_provider_as_one_byte}` |
| Contextual `n` with inherited provider/Location and idempotent independent creation | `navigator::view::tests::new_inherits_the_selected_workstreams_provider_and_location_context` and `actions::tests::{independent_creation_reuses_its_request_without_a_git_effect,independent_creation_keeps_the_project_root_without_touching_files,independent_creation_survives_one_provider_start_failure_without_fallback}` |
| Same-provider Codex/OpenCode Fork, transaction ordering, idempotency, recovery, and exact lost-result reconciliation | `actions::tests::{codex_fork_commits_before_start_and_reuses_the_request_without_a_second_fork,opencode_fork_commits_before_start_and_reuses_the_request_without_a_second_fork,codex_fork_reconciles_a_lost_result_without_retrying_the_provider,codex_fork_absent_reconciliation_enters_recovery_and_recovers_exactly_once,opencode_unknown_effect_is_terminal_and_never_retried,opencode_unattempted_recovery_records_attempt_before_effect_and_commits}` plus the provider adapter boundary tests |
| Park/Start, archive/restore, attention, and durable lifecycle routing | `runtime::tests::park_tolerates_a_private_server_that_is_already_gone`, `actions::tests::archive_and_restore_without_a_runtime_never_start_codex`, and `navigator::view::tests::{primary_action_uses_durable_lifecycle_not_runtime_guesswork,lifecycle_keys_emit_exact_reversible_action_revisions}` |
| Observer readiness, exact recovery, provisional cleanup/restart, and fail-closed ambiguity | `actions::tests::{completed_native_review_promotes_pending_observer_before_a_managed_action,native_recovery_uses_an_exact_binding_or_the_native_picker,opencode_recovery_handle_match_is_exact_and_provider_namespaced}` and `provisional::tests::{recovery_cancels_handoff_graph_cleans_artifacts_and_rejects_replay,recovery_preserves_live_unknown_foreign_and_changed_evidence}` |

A disposable Ubuntu 24.04 container copied the read-only source mount into
container-local storage, installed Rust 1.88.0, tmux 3.4, Bash, and Zsh, and ran
`cargo test --locked --all-targets --all-features`. The final corrected source
passed 350 library tests and 7 presentation integration tests. A separate
Rust 1.88/Debian/tmux 3.3a run passed the same matrix.

That clean-environment pass first exposed a tmux 3.4 startup race hidden by the
newer development tmux: a detached 80-column window could compress the
Navigator to one column while creating the preferred 96-column provider pane.
The candidate now starts detached presentations at the exact 129-by-24
two-pane geometry before splitting, with a unit command-contract test and the
disposable integration matrix passing.

The first ordinary 80-column attach then exposed a second, narrower race: the
installed tmux width hook reached the correct final 32/47 layout, but the
Navigator could inspect the transient attach topology and retain a stale error
banner. The original controller-only retry did not protect the equivalent
one-shot topology observation during presentation startup. The current
candidate shares one bounded 100-millisecond retry between startup and
post-attach restoration. Only `InvalidTopology` is retryable; unrelated errors
fail immediately and persistent incomplete topology still refuses. Three
deterministic tests prove transient recovery, persistent refusal, and immediate
unrelated-error refusal.

The missing startup coverage was later reproduced by both jobs in
[CI run 33292725378](https://github.com/byebyebryan/workstream-navigator/actions/runs/33292725378)
for traceability commit `08f9265`, including the accepted Rust
1.88/tmux 3.3a matrix. That falsification triggered correction `ed0d883`; it
does not retroactively turn the original controller-only retry into passing
evidence.

The post-acceptance correction also drives the production Fork/recovery action
and transactional state paths through deterministic provider-effect seams for
both providers. Those tests prove that the attempt marker precedes the provider
effect, destination commit precedes Runtime start, request replay cannot fork a
second provider session, Codex lost results reconcile without retry, and
OpenCode unknown effects become terminal. The obsolete test-only
`prepare_fork` seam is removed.

For this current candidate, `scripts/check` passed 357 library tests and 7
presentation integration tests together with formatting, strict Clippy,
packaging, dependency policy, semantic acceptance, and documentation checks. A
fresh Rust 1.88.0/Debian/tmux 3.3a container copied the read-only source mount
to container-local storage, deleted the copied build output, and passed the
full locked all-targets/all-features suite five consecutive times. The
correction is implemented in `ed0d883`; no remote-CI or
accepted-release/live-provider result is claimed for it in this record.
Per-host development installation is separate operational evidence.

## Clean-break correction

[Spike 0027](../spikes/0027-d18-root-move-falsification.md) proves that an
unprivileged online release tool cannot establish the required race-free
zero-arbitrary-holder boundary around a coherent online backup. `/proc` may be
incomplete and a point-in-time scan cannot prevent a new cwd, descriptor,
executable, or socket holder before rename.

That result rejects the stronger backup-and-rollback contract; it does not
require compatibility machinery for discarded WSNav state. D18 now uses an
explicit destructive reset: stop exact owned processes and private servers,
remove the exact D17.1 observer declaration while schema 14 remains available,
atomically quarantine the whole root as non-input discarded state, install the
accepted D18 artifact, and directly bootstrap schema 15. No migration, import,
adoption, or symmetric state rollback is permitted.

## Ordinary reset and installation evidence

The operator explicitly authorized discarding the D17.1 WSNav state. Read-only
preflight found two exact live Codex Runtimes and no presentation. Both Runtime
records matched their provider process and private tmux identity, and the
installed D17.1 `park` path stopped them before reset. Provider-native Codex
history remained outside the WSNav root.

The installed D17.1 `remove-observer` path then removed the exact owned
declaration completely. No provider-settings remainder remained. With the
presentation and Runtime directories empty and no exact WSNav-owned process or
private server live, the complete schema-14 root was atomically renamed to a
sibling quarantine as discarded data. D18 does not read that directory.

The byte-identical D18 release artifact was atomically installed and directly
bootstrapped ordinary state with application ID `0x57534e56`, schema 15, and
zero Workstreams, Runtimes, or operations. Bootstrap, database, and provisional
lock artifacts are private. The corrected first-launch presentation is healthy,
detached, and ready for the operator's next `wsnav` attach.

After acceptance, the exact discarded D17.1 quarantine was deleted. That
deletion is not recoverable. Provider-native history remains provider-owned
outside the WSNav state root. Unrelated historical backup and test roots were
not changed.

## Disposable live-provider acceptance

The operator separately authorized real Codex and OpenCode launches in
disposable state roots, repositories, provider homes, XDG directories, and
private tmux servers. Codex authentication and OpenCode authentication were
copied only into their respective disposable provider homes with mode `0600`.
They were never printed, parsed, hashed, or placed in WSNav state. No prompt,
response, tool output, terminal frame, scrollback, transcript, credential,
native session identifier, PID, UUID, endpoint, or temporary path is retained
in this record.

The accepted candidate used Codex CLI `0.150.0`, OpenCode `1.18.23`, and tmux
`3.7c`. The installed executable remained byte-identical to the release build
at SHA-256
`f732e2b16344b038cd05996501ce77be42302f7403de9720d156dbf24777d124`.

### Codex

The native Codex review displayed the candidate observer declaration and the
operator explicitly trusted all four declared hooks. WSNav then promoted the
provisional shell to one exact managed Codex Runtime. A completed native turn
produced exact lifecycle and sticky-result-attention evidence. Selecting the
replacement Shell created no additional durable Workstream, Runtime, binding,
or operation.

Park stopped the exact provider process, observer participation, managed tmux
server, and Runtime socket while retaining the native binding. Ordinary Start
then launched a new Runtime generation against that exact retained session.
Codex `0.150.0` did not emit `SessionStart(source=resume)` merely by displaying
the resumed TUI; the Runtime therefore truthfully remained `starting`. The
next native turn emitted exact same-session prompt and settled evidence and
returned the Runtime to result attention. This is accepted for ordinary Start;
the separate recovery-required route still requires an exact
`SessionStart(source=resume)` and remains fail closed without it. A final Park
and presentation exit removed every disposable Codex process, private server,
observer declaration, profile, and Runtime artifact.

### OpenCode

WSNav promoted the provisional shell to one exact blank root OpenCode session,
started the native TUI, and established a generation-bound ready observer. A
completed native turn produced one exact settled-message identity and sticky
result attention. Selecting the replacement Shell again left all durable
managed-state counts unchanged.

Park removed the provider process, ready observer handle, managed tmux server,
and Runtime socket while retaining the exact binding. Ordinary Start then
created a new Runtime generation against the same native session with
`start_source=resume`, a corroborated native provider process, and a ready
generation-bound observer. Because an already-idle resumed OpenCode session
emitted no new lifecycle event, the Runtime correctly remained `starting`
until a future native event rather than synthesizing provider state. Final Park
and presentation exit removed every disposable OpenCode process, listener,
observer handle, private server, and Runtime artifact.

Two harness-only retries occurred before any provider effect: one disposable
state-root name exceeded the private tmux socket-path limit, and one isolated
XDG configuration initially exposed the account shell's native prompt setup.
Neither attempt created a Workstream, Runtime, provider binding, operation, or
provider process. Both were classified as pre-provider harness failures and
fully deleted before the accepted run.

## Cleanup and commit binding

Both accepted disposable roots, repositories, provider homes, authentication
copies, presentations, Runtime directories, helpers, observers, endpoints, and
private tmux servers were deleted after exact shutdown checks. The ordinary
WSNav state retained its pre-acceptance aggregate parked/stopped state and ready
Codex integration, with no presentation or Runtime server. The user's default
tmux server contained no WSNav session. Accepted checkpoint `c961c7e` binds the
accepted source and evidence to installed SHA-256
`f732e2b16344b038cd05996501ce77be42302f7403de9720d156dbf24777d124`.
