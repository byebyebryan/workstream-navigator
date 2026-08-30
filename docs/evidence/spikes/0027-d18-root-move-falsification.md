# D18 root-move proof falsification

Date: 2026-08-29

Status: the proposed unprivileged coherent-backup/online-rollback contract is
falsified. This spike is historical design evidence; no release tool, provider
process, ordinary WSNav state, installation, or root move was created or
performed during the spike.

## Question

Can an external, unprivileged Linux release tool hold the exact provisional
lease and an exclusive SQLite quiescence fence, prove that no process can
retain or newly acquire any executable, cwd, file descriptor, or socket beneath
the WSNav root, and then rename that complete root without weakening D18's
fail-closed boundary?

## Result

No. The file, database, and rename portions are individually implementable:
private no-follow identity checks, `flock`, an exclusive SQLite connection,
Linux `renameat2(RENAME_NOREPLACE)`, and parent-directory `fsync` all have
bounded implementations. They do not compose into the required zero-holder
proof for an unprivileged online process.

Two independent gaps falsify the contract:

1. `/proc` is not complete authority. An unprivileged process can receive
   `EACCES` while reading another process's `exe`, `cwd`, `root`, or `fd`
   entries. Treating that uncertainty as absence would violate fail-closed;
   treating it as uncertainty makes the verifier refuse on ordinary hosts.
2. Even a complete point-in-time scan has a race. Neither a directory
   descriptor, `flock`, the provisional lease, nor a SQLite lock prevents an
   unrelated same-user process from opening the root, changing cwd into it, or
   creating a socket after the scan and before `renameat2`. The rename can
   succeed while that process retains authority over the moved tree.

A private checksummed manifest can make a multi-phase move recoverable, but it
cannot close either process-authority gap. Implementing only the checks that
are convenient would make the procedure look exact while weakening a core
invariant, so no root-moving tool was added.

## Original consequence

D18.0 through D18.2 and the semantic repository gates remain valid source
work. D18.3 reset, rollback, installed parity, and ordinary-root acceptance are
blocked at the design boundary. D17.1/schema 14 remains installed and no
ordinary-state action is authorized.

Continuing requires a new product decision with a separately reviewed proof
model, such as a privileged kernel-mediated freeze/deny-open boundary or an
offline environment where the state filesystem cannot be held by the running
user session. Relaxing the zero-holder requirement to a best-effort process
scan is not an implementation correction and was not assumed.

## Superseding clean-break decision

The product subsequently rejected the premise that D17.1 state must remain a
coherent rollback epoch. D18 uses a separately authorized destructive reset:
it establishes absence only for exact WSNav-owned processes and private tmux
servers, removes the owned observer declaration, and atomically quarantines
the complete old root as discarded non-input data before direct schema-15
bootstrap. No migration, adoption, restore, or state rollback route exists.

This spike therefore remains evidence against claiming arbitrary-holder
exclusion or recoverable online backup semantics. Its stronger proof obligation
is not a blocker for the destructive reset.
