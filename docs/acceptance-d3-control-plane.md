# D3 SSH Control-Plane Acceptance

Date: 2026-07-30

Status: pass — automated control-plane and bounded native-Codex acceptance

## Automated contract evidence

- Versioned requests and responses use one bounded JSON frame. Oversized and
  malformed input is drained and rejected without provider data.
- The local subprocess adapter exercises the exact hidden `_remote` command
  used by SSH, including revision-guarded attention acknowledgement.
- Fake-SSH tests prove fixed `ssh` argument vectors, safe target/executable
  validation, bounded rejection handling, and an interactive `ssh -tt`
  attachment path separate from control traffic.
- Client registrations persist host identity, registry generation, executable,
  transport, and capability fingerprint. Identity, generation, and capability
  drift reject action authority until explicit reset and re-registration.
- Navigator tests prove a disconnected remote host retains cached working and
  result-attention metadata instead of being projected as stopped. The UI
  labels cached state unavailable and backs off retries.
- Every automated test uses a disposable state root and subprocess. No test
  installs a Codex profile, launches a normal Codex session, or uses the
  default tmux server.

## Recorded native-Codex acceptance

- The operator used preinstalled, matching `wsnav` builds on local and remote
  hosts. Workstream Navigator neither copied nor updated the remote binary.
- Existing local and remote native Codex result tips remained visible and
  directly interactive through their respective provider panes.
- Parking the remote Runtime kept attachment and SSH diagnostics out of the
  provider pane. Selecting it again cold-resumed the exact native tip.
- A bounded transient-disconnect exercise retained cached remote attention and
  recovered it without changing durable remote state.
- Sanitized pre/post comparisons confirmed that ordinary tmux state was
  unchanged on both hosts. All managed Runtime and presentation state used
  private tmux servers.
- No acceptance-specific owned artifacts required cleanup. The normal remote
  installation remains user-owned.

The recorded fixture contains only sanitized relationships, capability checks,
and cleanup status. It excludes provider identifiers, prompts, responses,
paths, terminal output, process IDs, credentials, and raw provider payloads:
[D3 SSH control-plane fixture](../spikes/fixtures/d3-ssh-control-plane.json).
