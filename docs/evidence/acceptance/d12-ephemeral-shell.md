# D12 ephemeral Workstream shell acceptance

Status: local and real-SSH machine acceptance passed on 2026-08-15, followed
by local normal-environment operator confirmation and 2026-08-17 remote
normal-environment shell-launch plus installed automatic-switch confirmation.
Only the SSH completed-output preservation confirmation remains pending.

The deterministic and disposable automated gate is recorded in the D12
roadmap section. It proves the private tmux allowlists, pane-role and geometry
authority, local and remote launch vectors, control ABI, idempotence, literal
prefix delivery, cleanup, context retention, and ordinary-tmux isolation
without reading terminal content. This record is reserved for the remaining
operator-visible behavior; it must not be marked passing from automated
evidence alone.

## Operator boundary

The run requires explicit approval because it opens real provider sessions and
starts a real unprivileged loopback OpenSSH daemon. It uses the current
checkout's candidate executable rather than replacing the installed
executable. Local and SSH repositories, WSNav state, provider homes, SSH keys
and configuration, private tmux servers, and helper processes are disposable.
After explicit approval, the procedure may make one exact mode-preserving copy
of the ordinary provider authentication file into each disposable provider
home, following the established D8 production-acceptance boundary. It never
parses, logs, returns, or places that file in WSNav state, and cleanup must
remove every copy. If that exact disposable authentication setup is
unavailable, the run remains blocked rather than falling back to ordinary
provider state.

The acceptance procedure machine-checks pane metadata, lifecycle, cleanup, and
ordinary-tmux isolation while the operator confirms terminal-visible behavior
in the foreground. It records only versions, pass/fail assertions, and bounded
cleanup diagnostics. It does not capture or retain provider output, shell
output, commands, history, scrollback, terminal frames, prompts, responses,
credentials, repository paths, opaque identifiers, process command lines, or
raw SSH data.

## Required confirmation

For both one local Workstream and one Workstream reached through the real
loopback SSH endpoint, the operator confirms:

1. The native provider remains interactive and a completed result remains
   visible and unchanged before, during, and after utility-shell use.
2. `Ctrl+b "` creates one shell below the provider and focuses it. A second
   invocation focuses that same pane without creating or rearranging another
   pane; `Ctrl+b %` gives guidance without splitting.
3. Visual `hostname` and `pwd` inspection identify the expected launch host and
   canonical disposable ProjectLocation. A read-only `git status --short`
   behaves normally. None of their output is copied into this record.
4. Reselecting or reconnecting the same exact host and Workstream leaves its
   shell unchanged. Selecting a different Workstream automatically removes the
   exact utility without prompting, restores the two-pane layout before the
   provider changes, and never displays the new provider above the prior
   Workstream's shell. Provider input continues to reach only the selected
   provider pane.
5. Detaching and reattaching the same presentation retains the one live shell
   and its fixed context. Exiting it normally or with `Ctrl+d` immediately
   restores the exact two-pane layout without restarting WSNav.
6. Guarded `Ctrl+b x` closes only the focused utility shell. It cannot close or
   respawn the Navigator or provider panes.

## Cleanup gate

After both paths, every disposable Workstream is parked and its exact provider
process group is gone; all disposable private tmux sockets, provider roots,
WSNav roots, repositories, SSH keys/configuration, wrappers, and the loopback
daemon are removed. The ordinary tmux session inventory must exactly match its
pre-run snapshot. Any ambiguous identity, surviving process/root/socket,
captured terminal content, or ordinary-tmux difference falsifies the run and
leaves D12 incomplete.

## Machine acceptance evidence

An explicitly authorized run against the 2026-08-15 checkout exercised both
the local path and a real disposable loopback-SSH endpoint. Every then-current
non-visual local and SSH assertion passed: ABI 2 preflight, native provider
interactivity, running attachment identity, below-provider geometry, one-shell
idempotence, canonical cwd, read-only Git inspection, provider switching with
fixed utility context and unchanged Runtime identity, detach/reattach, normal
shell-exit cleanup, guarded close, bounded evidence, exact cleanup, and
ordinary-tmux non-interference. The observed `wsnav`, tmux, and SSH tools were
all marked `checked`; the cleanup result was `complete`; no disposable
acceptance root remained. The fixed-context switching assertion is historical
and was superseded by the simpler automatic-close contract above; it cannot
close the current switching gate.

The run captured no provider, shell, terminal, or raw SSH content. Because it
was driven without viewing those native terminal surfaces, it deliberately
left `local_completed_output_preserved` and
`ssh_completed_output_preserved` false. D12 therefore remains blocked on
operator visual confirmation rather than treating machine metadata as proof of
unchanged completed output.

The operator subsequently tested the installed local ABI 2 build from a normal
account environment and confirmed the utility shell used the expected zsh
profile, normal `exit` cleanup restored the provider geometry, guarded
`Ctrl+b x` closed the utility, and the local completed provider surface
remained usable. This closes the local visual gate without retroactively
changing the content-free machine result.

The first normal-environment SSH follow-up exposed two fail-closed launch
regressions that the disposable fixture had not exercised: an exact live
Runtime could still be durably `Starting` while lifecycle hooks were pending,
and an SSH command environment need not export `SHELL`. The remote helper now
accepts `Starting` only after the existing exact live-process preflight and
resolves the effective account's login shell from the system account database.
A bounded content-free probe then remained live until its diagnostic timeout,
cleaned up completely, and the operator confirmed the installed remote shell
opened and remained usable. This confirms the corrected SSH shell-launch path;
the separate visual assertion that completed provider output remains unchanged
is still pending.

The subsequent V1 simplicity decision keeps Workstream switching free of a
confirmation step: switching to a different Workstream closes the exact
utility before provider replacement, while same-Workstream reactivation keeps
it. Focused disposable coverage proves that transition, including fail-closed
ambiguous-pane and extra-window cases. The operator then confirmed the
installed local presentation closes the shell on a different-Workstream
selection, restores the two-pane layout, does not restore the old shell when
returning, and preserves the shell on same-Workstream reselection. No terminal
content was retained from that confirmation. The separate SSH completed-output
visual assertion remains the only open D12 gate.
