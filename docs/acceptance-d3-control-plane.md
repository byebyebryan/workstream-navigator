# D3 SSH Control-Plane Acceptance

Date: 2026-07-30

Status: automated control-plane acceptance passed; live remote-Codex acceptance pending

## What passed

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

## Remaining live gate

The designated SSH target is reachable but does not yet have a user-installed
`wsnav` executable. That is an expected V1 precondition, not a deployment
failure: Workstream Navigator must not copy, bootstrap, or update the remote
binary. After the operator installs it and completes the remote observer setup,
the pending acceptance is one bounded start/attach/complete/park/resume run.

The live record must prove that both hosts keep their provider result tips,
their default tmux servers remain unchanged, and a disconnect leaves remote
attention durable. It must contain only sanitized relationships, event order,
capability checks, timings, and cleanup status—never raw IDs, prompts,
responses, paths, terminal output, process IDs, or credentials.
