# D1 local native-Codex acceptance

Status: pass — 2026-07-29

This is the final acceptance procedure for the D1 checkpoint. It exercises a
real Codex TUI while preserving the user's ordinary tmux server and removing
every Workstream Navigator-owned test artifact afterward.

## Authority and retained state

The operator must explicitly approve Codex's native hook review. Do not use
`--dangerously-bypass-hook-trust` and do not write Codex's trust store.

The procedure intentionally uses the normal `CODEX_HOME`: the native trust
decision and the one Codex test thread are Codex-owned state. Workstream
Navigator does not delete either. The temporary Git repository, WSNav state
root, private tmux runtime, and owned observer profile are removed at cleanup.
Codex may append its native hook and project-trust records to that dedicated
observer profile during review; WSNav accepts only that bounded native suffix
and removes it only together with the exact dedicated profile.

## Procedure

1. Build the current checkout and create an empty temporary Git repository.

   ```console
   cargo build
   acceptance_root="$(mktemp -d)"
   git init "$acceptance_root/repository"
   git -C "$acceptance_root/repository" config user.name wsnav-acceptance
   git -C "$acceptance_root/repository" config user.email wsnav@example.test
   git -C "$acceptance_root/repository" commit --allow-empty -m initial
   ordinary_tmux_before="$(env -u TMUX tmux list-sessions -F '#{session_name}:#{session_created}:#{session_windows}' -O name 2>/dev/null || true)"
   ```

2. Install the profile into the normal `CODEX_HOME`. `setup` opens one native,
   profile-selected review TUI in a private WSNav tmux server and an empty
   disposable cwd. Review and trust the exact `wsnav _hook` command in Codex's
   native `/hooks` UI, then exit without submitting a prompt. It neither uses
   the default tmux server nor writes Codex's trust state itself.

   ```console
   target/debug/wsnav --state-root "$acceptance_root/state" setup
   target/debug/wsnav --state-root "$acceptance_root/state" trust-observer
   ```

3. Register and start the external checkout. Attach to the private runtime,
   submit one harmless prompt, and wait for its normal result. Do not use a
   manager-owned prompt surface.

   ```console
   registration="$(target/debug/wsnav --state-root "$acceptance_root/state" register "$acceptance_root/repository")"
   workstream_id="${registration##* }"
   target/debug/wsnav --state-root "$acceptance_root/state" start "$workstream_id"
   target/debug/wsnav --state-root "$acceptance_root/state" attach "$workstream_id"
   ```

4. In a separate terminal, prove status/rename do not type into or redraw the
   provider pane. In the directly attached Codex pane, invoke native `/clear`,
   then submit one harmless destination prompt and wait for its normal result.
   This is the only D1.5 native same-Workstream cutover acceptance: the TUI and
   private Runtime must remain in place while the bound ConversationTip changes.
   Then park and exact-resume the current thread.

   ```console
   target/debug/wsnav --state-root "$acceptance_root/state" status "$workstream_id"
   target/debug/wsnav --state-root "$acceptance_root/state" rename "$workstream_id" "D1 acceptance"
   # In the attached native Codex TUI: /clear, then one harmless prompt.
   target/debug/wsnav --state-root "$acceptance_root/state" status "$workstream_id"
   target/debug/wsnav --state-root "$acceptance_root/state" park "$workstream_id"
   target/debug/wsnav --state-root "$acceptance_root/state" start "$workstream_id"
   target/debug/wsnav --state-root "$acceptance_root/state" attach "$workstream_id"
   ```

   Before park, `status` must report `private runtime: live`, `provider
   binding: bound`, and `result attention: unseen`. The pre-clear attention
   remains sticky even though the current tip is now the cleared destination.
   Status deliberately exposes no session ID, process ID, cwd, hook payload,
   or terminal content.

5. Park, remove only the exact owned profile, compare the ordinary tmux
   fingerprint, and remove the temporary filesystem artifacts.

   ```console
   target/debug/wsnav --state-root "$acceptance_root/state" park "$workstream_id"
   target/debug/wsnav --state-root "$acceptance_root/state" remove-observer
   ordinary_tmux_after="$(env -u TMUX tmux list-sessions -F '#{session_name}:#{session_created}:#{session_windows}' -O name 2>/dev/null || true)"
   test "$ordinary_tmux_before" = "$ordinary_tmux_after"
   rm -rf -- "$acceptance_root"
   ```

## Pass criteria

- The provider pane remains directly interactive, and the completed result is
  unchanged until an operator action.
- Exactly one dedicated private tmux server/session/window/pane exists for the
  Runtime; the ordinary tmux fingerprint is unchanged.
- The first native result creates sticky attention and the exact session resumes
  after park.
- Rename affects the native Codex thread name and no profile or state file
  remains after exact removal.
- The recorded finding contains only pass/fail assertions and timing; it does
  not commit prompts, responses, terminal capture, UUIDs, paths, PIDs,
  credentials, or raw hook/App Server payloads.

## Final recorded result

The final bounded native run passed with Codex CLI `0.145.0` and the
`codex-d1-private-runtime-hooks-v1` contract fingerprint.

- Native `/hooks` approval was explicit and no trust bypass was used.
- The single native turn completed through a directly interactive private
  provider pane. Its accepted lifecycle reached a bound session with durable,
  visible unseen-result attention only after the provider pane/process/cwd and
  process-birth/short-ancestry evidence agreed.
- Status and ephemeral App Server rename ran outside the provider pane; the
  user confirmed the preserved normal result and canonical thread title after
  exact park/resume.
- The exact dedicated observer profile, managed runtime, review runtime, and
  disposable state/repository were removed. The ordinary tmux fingerprint was
  unchanged before and after the run.
- The sanitized [fixture](../spikes/fixtures/d1-local-codex-acceptance.json)
  contains only booleans and a provider contract fingerprint. No identities,
  prompt/result text, paths, PIDs, credentials, raw hook payloads, App Server
  frames, or terminal capture were committed.

## 2026-07-29 preliminary live run

The bounded live happy path completed: native trust review, a single harmless
native prompt and result, out-of-band name change, exact park/resume, native
result preservation, exact profile removal, private-runtime cleanup, and an
unchanged ordinary tmux fingerprint. The run revealed and corrected three
implementation defects before final cleanup:

- Codex appends its native hook/project trust records to the selected profile;
  ownership validation now permits only that schema-checked suffix.
- The generated hidden hook spelling did not match the CLI parser; the exact
  private entrypoint is now parseable and covered by a regression test.
- The short-lived App Server client closed stdin before the real server had
  necessarily dispatched the queued action; it now waits for the exact result
  before shutdown.

That run was intentionally not the final D1 pass record: the follow-up
hardened hook provenance against the live provider process and added sanitized
attention visibility. The final recorded result above is the fresh rerun using
that implementation and complete cleanup.
