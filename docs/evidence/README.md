# Historical evidence

This directory preserves sanitized, dated evidence for decisions that led to
the current V1 design. It is deliberately separate from the present-tense
product contract:

- [Design](../design.md) defines product and architecture.
- [Roadmap](../roadmap.md) defines delivery status and acceptance gates.
- The evidence below records the exact candidate, environment, procedure, and
  limitations stated in each file. Its historical versions, test counts, and
  UI details are not current behavior by themselves.

## D3-D15 SSH and remote evidence is historical

D3 through D15 were completed against earlier product surfaces. Any SSH,
remote-host, cross-host, host-registration, remote-attachment, or combined
catalog behavior described by those checkpoints is historical evidence for the
candidate that was tested. D16 retires WSNav-managed SSH and cross-host
operation from the current contract: the supported multi-host composition is
ordinary operator SSH followed by a host-local `wsnav` instance, with separate
terminal windows per host. The historical files are intentionally not rewritten
to make their old procedures appear current.

## Acceptance records

- [D1 local Codex](acceptance/d1-local-codex.md)
- [D2 local navigator](acceptance/d2-local-navigator.md)
- [D3 SSH control plane — historical; retired by D16](acceptance/d3-control-plane.md)
- [D4 independent and forked Workstreams](acceptance/d4-workstreams.md)
- [D5 V1 closure](acceptance/d5-v1-closure.md)
- [D5.1 operational closure](acceptance/d5.1-operational-closure.md)
- [D5.2 correctness closure](acceptance/d5.2-correctness-closure.md)
- [D6 source-installed operator beta](acceptance/d6-operator-beta.md)
- [D6.1 project identity](acceptance/d6.1-project-identity.md)
- [D7 navigator workflow](acceptance/d7-navigator-workflow.md)
- [D8.1 real multi-provider acceptance](acceptance/d8.1-multi-provider.md)
- [D8.2 OpenCode Fork and recovery acceptance](acceptance/d8.2-opencode-fork-recovery.md)
- [D12 ephemeral Workstream shell](acceptance/d12-ephemeral-shell.md)
- [D16 host-local simplification](acceptance/d16-host-local.md)

## Design spikes

The [spikes](spikes/) establish the narrow tmux, remote attachment, native
Codex presentation, observer, naming, and settled-fork boundaries. They are
falsification studies, not product documentation. [Spike
0014](spikes/0014-terminal-fidelity-a-b.md) adds the deterministic A/B
instrument for the deferred terminal-fidelity cursor amplification; [Spike
0018](spikes/0018-navigator-input-latency.md) separates local synthetic input
delivery from presentation echo under static and 10 FPS Navigator panes; [Spike
0015](spikes/0015-opencode-provider-feasibility.md) records the
opencode provider fork-exactness, fork-lineage, and shared-database
concurrency probes; [Spike
0016](spikes/0016-opencode-runtime-contract.md) records the native TUI
Runtime, observer, and exact HTTP Fork boundary; and [Spike
0017](spikes/0017-opencode-fresh-session.md) records blank-session binding,
endpoint ownership, and per-Runtime observer sidecar evidence.
[Spike 0019](spikes/0019-brokered-onboarding-shell.md) records the bounded
brokered provisional-shell topology study and its implementation limits.
[Spike 0020](spikes/0020-opencode-1.18.23-revalidation.md) records the
OpenCode `1.18.23` revalidation of the historical fresh-session contract.
[Spike 0021](spikes/0021-d17-two-phase-handshake.md) validates the narrow D17
prepare-capability-helper-exec topology across synthetic Bash/Zsh and
Codex/OpenCode routes while preserving the remaining D17.0 acceptance gates.
[Spike 0022](spikes/0022-d17-account-shell-wrapper.md) validates the controlled
non-login Bash/Zsh account-wrapper candidate and records the mandatory Bash
login preflight boundary.
[Spike 0023](spikes/0023-d17-provisional-lock.md) validates the isolated
schema-14 stable `provisional.lock` installation and refusal lifecycle while
leaving the cross-actor onboarding races as D17.0 work.

## Provider studies

The [studies](studies/) directory records focused provider-contract research
used to make the design conservative and reproducible. [Study
0004](studies/0004-herdr-v0.8-comparison.md) is the competitive-positioning
exception: it compares the released V1 against Herdr 0.8.0 as documentation
research and changes no product boundary.
