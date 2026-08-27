# Spike 0021: D17 two-phase launch handshake

## Hypothesis / question

Is the narrow D17 shell-to-provider handoff mechanically feasible in both
Bash and Zsh: controlled provider function, bounded prepare child, opaque
one-shot capability, shell-replacing hidden helper, atomic consume under a
CLOEXEC lease, and final provider `exec` without changing the shell leader's
process identity?

This follows [Spike 0019](0019-brokered-onboarding-shell.md), which selected a
controlled function plus `exec` but did not implement the two-phase handshake.
It is a falsification probe for that missing seam, not D17.0 acceptance.

## Procedure and isolation

The deterministic harness is
[`spikes/d17-two-phase-handshake.py`](../../../spikes/d17-two-phase-handshake.py),
with its sanitized [fixture][fixture]. Run it with:

```text
python3 spikes/d17-two-phase-handshake.py \
  --result spikes/fixtures/d17-two-phase-handshake.json
```

The harness creates a mode-`0700` temporary root, one disposable non-bare Git
repository, fixed private shell startup files, and a fresh private tmux socket
for each Bash/Zsh and Codex/OpenCode route. It never starts a real provider.
The controlled function invokes the prepare broker as a direct shell child over
captured stdout; stdout contains only the opaque capability. The function then
`exec`s the hidden helper with the same provider arguments. The helper
revalidates all synthetic bound claims, consumes the verifier-backed
capability, marks synthetic ownership `runtime_owned`, retains a validated
non-inheritable lock FD, and `exec`s a fixed fake provider.

The fake provider records transient process topology, cwd, arguments, and open
FD evidence, prints one bounded provider marker, and waits for Ctrl-C. Those raw
records and the capability store are deleted with the temporary root. The
committed fixture retains only booleans, provider/shell enums, contract labels,
and tool versions. The ordinary tmux server is read only for a before/after
fingerprint.

The same capability store seam separately exercises replay, expiration,
invalid verifier, mutation of every bound claim, reserved grammar before
issuance, and an intentionally inheritable lease FD. Rejection reasons are
bounded; mismatch and verifier failures leave the capability unconsumed.

## Result

The fixture passed on Bash `5.3.15`, Zsh `5.9.2`, and tmux `3.7c`.

| Route | Prepare direct child | Helper kept PID/birth | Provider kept PID/birth/PGID/session | Args and cwd kept | Lease absent after exec |
| --- | --- | --- | --- | --- | --- |
| Bash → Codex | yes | yes | yes | yes | yes |
| Bash → OpenCode | yes | yes | yes | yes | yes |
| Zsh → Codex | yes | yes | yes | yes | yes |
| Zsh → OpenCode | yes | yes | yes | yes | yes |

Each route detected the nested Git root from the shell cwd, bound cwd/root,
provider, argv digest, shell process identity, and the complete synthetic
request/presentation/slot/lease/Runtime/revision context. Durable synthetic
ownership preceded fake-provider exec. Only a verifier was stored; the live
token was returned through the function's private non-terminal command
substitution. The helper confirmed the lease FD was CLOEXEC, and the fake
provider confirmed that descriptor was not inherited.

All fail-closed probes passed: a consumed token could not replay, an expired
token could not consume, an invalid verifier did not consume, mutating any one
bound claim did not consume, reserved grammar issued no capability, and an
inheritable lease FD was rejected before provider exec. All private servers and
temporary state cleaned up, and the ordinary tmux fingerprint was unchanged.

## Consequence

The exact two-phase topology is feasible and no longer needs to be treated as
an architectural unknown. D17.0 can implement this boundary without adding a
resident supervisor, changing the provider's pane-process identity, or leaking
the ownership lease into the provider.

The spike also supports keeping provider and directory choice native to the
provisional shell: the explicit `codex` or `opencode` command selects the fixed
route, while the broker binds the shell's actual cwd and containing Git root.

## Deliberate limits

- The capability record and ownership transition are synthetic files, not the
  schema-14 SQLite graph, `CompoundOperation`, marker, or onboarding journal.
- The lock is precreated inside disposable state. The spike does not prove
  schema-13-to-14 cutover, pending/ready lock installation, stable inode
  recovery, multiple presentations, or participant races.
- Expiry uses one immediate same-boot monotonic clock. Restart-safe boot-scoped
  expiry representation and reconciliation remain an implementation gate.
- The startup files are controlled wrappers. They do not reproduce and compare
  the full Bash/Zsh account startup graph, `HOME`/`ZDOTDIR`, aliases, functions,
  options, prompt readiness, aborts, or wrapper replacement.
- The admitted argument tuple and provider are synthetic. Real versioned Codex
  and OpenCode grammar, observer/preparation effects, OpenCode `POST /session`,
  native terminal behavior, exec failure, cancellation, crash recovery, and
  completed-output retention remain D17.0 gates.
- The probe does not implement the card promotion/replacement lifecycle or
  prove attachment/action fencing around `runtime_owned_launching`.

## Status

**Two-phase topology validated; D17.0 still needs implementation acceptance.**

[fixture]: ../../../spikes/fixtures/d17-two-phase-handshake.json
