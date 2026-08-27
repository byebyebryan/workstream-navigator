# Spike 0020: OpenCode 1.18.23 fresh-session revalidation

## Question

Does the existing [OpenCode fresh-session study](0017-opencode-fresh-session.md)
still pass against the installed OpenCode `1.18.23`, without changing its
historical `1.18.11` pin or the production contract?

## Procedure and isolation

This was a bounded, operator-approved live revalidation of the historical
study, not a new provider or broker implementation. The exact disposable-copy
procedure was:

1. Create a mode-0700 outer temporary directory.
2. Copy `spikes/opencode-fresh-session.py` and
   `spikes/opencode_support.py` into it.
3. Use `apply_patch` on only the temporary study copy to change exactly the
   single `VERSION = "1.18.11"` constant to `VERSION = "1.18.23"`, then verify
   that no other source difference existed.
4. Run the temporary copy with
   `--confirm-live-opencode --result <outer-temp>/opencode-1.18.23.json` under
   a bounded timeout.

The existing study supplied the isolation contract: separate XDG roots,
the established mode-0600 temporary auth-cache copy, two harmless fixed marker
prompts, private tmux sockets, and cleanup of provider/observer state. The
normal provider home, history, configuration, credentials, and tmux server
were not changed. The study's raw result was further sanitized to retain only
bounded status, reason, version, assertion booleans, and digest counts.

## Result

The installed OpenCode version was `1.18.23`. The bounded run completed in
approximately 17.4 seconds with:

- status: `pass`;
- reason: `blank-native-binding-and-sidecar-ownership-confirmed`;
- 23 of 23 named assertions passed: `operator_confirmed`,
  `production_command_has_no_pure_or_model_flags`,
  `version_allowlist_enforced`, `blank_session_precreated_without_messages`,
  `two_blank_same_root_sessions`, `native_tui_server_ready`,
  `exact_blank_session_visible`, `endpoint_process_correlation`,
  `wrong_healthy_endpoint_rejected`, `port_collision_rejected`,
  `stale_saved_port_rejected`, `observer_started_before_native_input`,
  `observer_generation_and_birth_recorded`,
  `observer_filtered_to_exact_session`,
  `observer_discarded_foreign_events`, `child_session_events_ignored`,
  `unrelated_root_session_not_selected`, `native_prompts_remain_non_crossing`,
  `observer_helper_crash_detected`, `observer_replacement_reconnects`,
  `detach_reopen_retains_runtime_and_observer`,
  `exact_resume_after_runtime_restart`, and `cleanup_complete`;
- 2 session digests and 2 generation digests were produced, with all digest
  values discarded; and
- the ordinary tmux fingerprint was unchanged and complete cleanup passed.

The further-sanitized [fixture][fixture] records these booleans and counts;
it contains no provider IDs, digest values, prompts, responses, terminal
captures, paths, credentials, or raw payloads.

## Scope and limits

This revalidates the historical OpenCode blank-session binding, endpoint
ownership, native prompt separation, observer generation/replacement, detach
and reopen, exact resume, and cleanup contract on `1.18.23`. It does not add
`1.18.23` to the historical `1.18.11` allowlist, change Spike 0017 or its
fixture/script, approve a production dependency, or authorize a design change.

It is also not evidence that the as-yet-unimplemented brokered provisional
shell can onboard a provider. Provider selection, broker reservation before
native launch, shell bypass handling, cwd/root policy, and the current
two-layer completed-output contract remain separate implementation and
acceptance work.

## Status

**Historical provider-contract revalidation passed; no product or design
approval.**

[fixture]: ../../../spikes/fixtures/opencode-fresh-session-1.18.23.json
