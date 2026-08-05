# Historical evidence

This directory preserves sanitized, dated evidence for decisions that led to
the current V1 design. It is deliberately separate from the present-tense
product contract:

- [Design](../design.md) defines product and architecture.
- [Roadmap](../roadmap.md) defines delivery status and acceptance gates.
- The evidence below records the exact candidate, environment, procedure, and
  limitations stated in each file. Its historical versions, test counts, and
  UI details are not current behavior by themselves.

## Acceptance records

- [D1 local Codex](acceptance/d1-local-codex.md)
- [D2 local navigator](acceptance/d2-local-navigator.md)
- [D3 SSH control plane](acceptance/d3-control-plane.md)
- [D4 independent and forked Workstreams](acceptance/d4-workstreams.md)
- [D5 V1 closure](acceptance/d5-v1-closure.md)
- [D5.1 operational closure](acceptance/d5.1-operational-closure.md)
- [D5.2 correctness closure](acceptance/d5.2-correctness-closure.md)
- [D6 source-installed operator beta](acceptance/d6-operator-beta.md)
- [D6.1 project identity](acceptance/d6.1-project-identity.md)
- [D7 navigator workflow](acceptance/d7-navigator-workflow.md)

## Design spikes

The [spikes](spikes/) establish the narrow tmux, remote attachment, native
Codex presentation, observer, naming, and settled-fork boundaries. They are
falsification studies, not product documentation. [Spike
0014](spikes/0014-terminal-fidelity-a-b.md) adds the deterministic A/B
instrument for the deferred terminal-fidelity cursor amplification, and
[Spike 0015](spikes/0015-opencode-provider-feasibility.md) records the
opencode provider fork-exactness, fork-lineage, and shared-database
concurrency probes.

## Provider studies

The [studies](studies/) directory records focused provider-contract research
used to make the design conservative and reproducible. [Study
0004](studies/0004-herdr-v0.8-comparison.md) is the competitive-positioning
exception: it compares the released V1 against Herdr 0.8.0 as documentation
research and changes no product boundary.
