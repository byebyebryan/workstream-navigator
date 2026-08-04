# Study 0004: Herdr 0.8.0 competitive comparison

Date: 2026-08-03

Status: research and product-positioning study; records how the released V1
Workstream Navigator compares with the current Herdr after the 0.8.0 release.
No Herdr dependency, integration, or product-boundary change is approved.

## Question

The user has seen recent Herdr promotions and product hot takes. How does the
shipped V1 Workstream Navigator compare with the current Herdr, and does any
Herdr capability contradict or overlap the accepted V1 boundary?

## Answer

Herdr and Workstream Navigator are adjacent but not head-to-head competitors:
Herdr is an agent-aware terminal multiplexer that owns the terminal substrate
(PTYs, terminal emulator, workspaces, tabs, panes, a socket API, plugins, and
a broad cross-provider agent-detection matrix), while Workstream Navigator is a
thin navigator that owns workstream organization, exact provider binding
authority, and compound lifecycle actions on top of tmux and the native Codex
TUI. Herdr's 0.8.0 release widened the gap on its side of that boundary and
left the V1 boundary untouched.

The earlier [Herdr assessment](../../design.md#evidence-basis)
(2026-07-24, Herdr 0.7.5) already reached the same structural conclusion:
Herdr is a UX reference and a possible future optional host, not the V1
substrate. The 0.8.0 comparison below confirms that finding is unchanged and
adds two independent convergences in Herdr's own changelog.

## Sources and method

Assessment is against the current public Herdr state as of 2026-08-03:

- upstream [herdrdev/herdr](https://github.com/herdrdev/herdr) on the `master`
  branch (the repository moved from `ogulcancelik/herdr`; 1,310 commits,
  ~24.1k stars, Apache-2.0 license after the 0.8.0 relicensing from
  AGPL-3.0-or-later);
- the `0.8.0` release (2026-08-03) changelog;
- the public [Herdr documentation](https://herdr.dev/docs/): concepts,
  session-state, integrations, agent-skill, plugins, socket-api,
  persistence-remote, and agents pages; and
- the previous [Herdr assessment](https://github.com/byebyebryan/agent-switchboard-python-reference/blob/main/docs/herdr-assessment.md)
  recorded against Herdr 0.7.5 at commit `b56baee9`.

The comparison is documentation-based; no Herdr binary was installed, no Herdr
or Codex state was touched, and no live side-by-side run was executed. Where
the docs are silent on a point, the study says so explicitly instead of
inferring a capability.

## Where the products overlap

| Topic | Herdr 0.8.0 | Workstream Navigator V1 |
| --- | --- | --- |
| Keep the native Codex TUI directly interactive | yes | yes (V1 tenet 1) |
| Detach/reattach while the agent keeps running | yes, via its own persistent server | yes, via one private tmux server per Runtime |
| Native provider resume of a known session | `codex resume <id>` on restored panes | `codex -C <project-root> resume <id>` with launch-correlated binding |
| "Done until the user views it" attention | `done` status marks in the agent panel and rollups | sticky per-Workstream `AttentionState` |
| Working-status display | static workspace marks since 0.8.0 (continuous spinners removed) | deterministic working indicator rendered only while a working row is visible |
| Codex `/new` does not take over a pane's session | 0.8.0 fix: nested/ephemeral Codex sessions no longer replace the owning pane's resumable session | Spikes 0011-0013: native `/new` is unsupported in a managed Runtime |

Two convergences stand out because they were not borrowed from the other side:

1. Herdr 0.8.0 removed continuous spinner rendering in favor of static agent
   status marks at the same time Workstream Navigator removed its continuous
   runtime/presentation control probes and gated its working spinner to rows
   that are actually working.
2. Herdr 0.8.0 fixed "nested or ephemeral Codex sessions no longer replace the
   owning pane's resumable session", which matches the fail-closed boundary
   Workstream Navigator recorded in Spikes 0011-0013: a distinct native `/new`
   thread cannot be exact-bound by passive evidence.

These are independent evidence that the underlying provider behavior is real
and that both products reacted conservatively.

## Where Workstream Navigator remains differentiated

The same gaps the 0.7.5 assessment identified remain open in 0.8.0:

- **Exact binding authority.** Herdr derives Codex lifecycle state from
  terminal-screen/OSC heuristics (screen-manifest detection) and session
  identity from a socket report; Workstream Navigator requires a direct-parent
  PID plus process birth plus cwd match, a transactional ProviderBinding, and
  App Server `thread/read` corroboration, and treats screen text and
  `thread/list` ordering as observation only, never authority.
- **Semantic lineage.** Herdr's hierarchy is Workspace -> Tab -> Pane with one
  stored native session reference; it has no Project -> Workstream -> Task ->
  ProviderThread lineage, no settled-turn conversation fork, no exact-once
  `thread/fork(lastTurnId)` request, and no compound-operation journal.
  Workstream Navigator's durable state model is built on those.
- **Result preservation and privacy.** Herdr persists optional pane history and
  now auto-reads text history for idle alternate-screen agents; its only
  "until the user acts" guarantee is a `done` attention mark. Workstream
  Navigator never captures terminal or provider content, never persists
  transcripts, and preserves a completed provider result byte-for-byte until
  the user acts.
- **Lifecycle idempotency.** Herdr's `agent prompt --wait` observes lifecycle
  state rather than one exact submitted turn. Workstream Navigator commits
  settled-turn, status, and sticky attention atomically under a transaction
  revision.
- **Multi-host model.** Herdr is one client attached to one remote Herdr
  server; the docs contain no unified multi-host resource tree, catalog
  synchronization, or cross-host project grouping. Workstream Navigator's
  versioned JSON protocol addresses local and SSH hosts through one interface,
  with client-side repository-fingerprint grouping beneath host-owned
  authority.
- **Trust boundary.** Herdr plugins run unsandboxed as ordinary user processes
  with the full Herdr CLI available and no per-action capability or bearer
  authority; Herdr relies on foreground-agent and source/sequence checks.
  Workstream Navigator has no out-of-band agent/plugin control surface and
  bounds mutation to launch-correlated, corroborated explicit actions.

## Where Herdr is stronger

Workstream Navigator deliberately does not compete here:

- **Provider breadth.** Herdr detects 17+ coding agents with evidence-based
  manifests; Workstream Navigator is Codex-only by explicit V1 scope.
- **Terminal substrate.** Herdr owns PTYs, a terminal emulator, scrollback,
  copy mode, mouse/drag interaction, keyboard and mouse as first-class
  controls, plugins and a marketplace, session handoff, and Windows support.
  Workstream Navigator reuses tmux and has no such surface by design.
- **Distribution and momentum.** Herdr ships via curl, Homebrew, mise, Nix, and
  Windows installers and is sponsor-funded full-time; Workstream Navigator is a
  source-installed `0.1.0` operator beta with no release channel.

## Limitations

This is a documentation comparison at one point in time. It did not run Herdr,
audit its source at a pinned commit, or exercise a live two-pane layout beside
it. Herdr's session-state, integration, and plugin contracts move quickly; a
capability asserted here can change in a later release. Herdr's Codex
integration installs a session-reporting hook, but the public docs do not
enumerate its accepted session-start sources or explicitly distinguish a
Codex `/new` thread from a `/clear` cutover, so that specific comparison is
limited to what the changelog and screen-manifest documentation state.

## Product decision

No product-boundary change is implied. Herdr remains a UX reference and a
possible future optional presentation host per the earlier assessment; it is
not the V1 substrate and no integration work is scheduled. Herdr 0.8.0
converging on static status marks and the nested-Codex-session boundary
validates the conservative direction already implemented in V1.

## Sources

- [herdrdev/herdr](https://github.com/herdrdev/herdr)
- [Herdr 0.8.0 changelog](https://github.com/herdrdev/herdr/blob/master/CHANGELOG.md)
- [Herdr documentation](https://herdr.dev/docs/)
- [Prior Herdr assessment (0.7.5)](https://github.com/byebyebryan/agent-switchboard-python-reference/blob/main/docs/herdr-assessment.md)
