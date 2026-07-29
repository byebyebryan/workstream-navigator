# D1 local native-Codex acceptance

Status: operator-run gate; not yet recorded as passed

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

2. Install the profile into the normal `CODEX_HOME`, then review and trust the
   exact `wsnav _hook` command in Codex's native `/hooks` UI. Do not submit a
   prompt during that review.

   ```console
   target/debug/wsnav --state-root "$acceptance_root/state" setup
   codex --profile wsnav-observer -C "$acceptance_root/repository"
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
   provider pane, then park and exact-resume the thread.

   ```console
   target/debug/wsnav --state-root "$acceptance_root/state" status "$workstream_id"
   target/debug/wsnav --state-root "$acceptance_root/state" rename "$workstream_id" "D1 acceptance"
   target/debug/wsnav --state-root "$acceptance_root/state" park "$workstream_id"
   target/debug/wsnav --state-root "$acceptance_root/state" start "$workstream_id"
   target/debug/wsnav --state-root "$acceptance_root/state" attach "$workstream_id"
   ```

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
