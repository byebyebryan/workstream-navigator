# Spike 0023: D17 schema-14 provisional lock

## Question

Can the D17 stable host-private `provisional.lock` be installed through the
specified schema-14 `pending` to `ready` lifecycle, then safely reused after a
holder crash without accepting a pre-schema, missing, symlinked, replaced, or
unlink-recreated artifact?

This isolates the durable lease primitive that D17 materialization, cleanup,
broker preparation, helper consume, and recovery will share. It is not the
onboarding implementation.

## Procedure and isolation

The deterministic harness is
[`spikes/d17-provisional-lock.py`](../../../spikes/d17-provisional-lock.py),
with its sanitized [fixture][fixture]. It creates a mode-`0700` temporary root
and one private SQLite database per case. It starts no provider, touches no
ordinary WSNav state, and only reads the ordinary tmux fingerprint before and
after the study.

The miniature schema starts at `13`. Migration first refuses any existing
`provisional.lock`; otherwise it transactionally records schema `14` and the
host-local lease as `pending` before the file exists. The installer then uses
create-new/no-follow, mode `0600`, bounded current-owner content, file and
directory `fsync`, descriptor/path device-inode comparison, and a nonblocking
exclusive CLOEXEC lease before recording `ready`.

Separate disposable cases leave an exact file in the post-create/pre-ready
crash window; hold and kill a child lock owner; and mutate a ready path through
missing, symlink, replacement, and unlink-recreate states. The committed
fixture retains only booleans, the contract label, and local Python/tmux
versions; the database, HostIds, paths, inodes, child identifiers, and lock
content are deleted with the temporary root.

## Result

The fixture passed on Python `3.14.7` and tmux `3.7c`.

- An artifact discovered before schema-14 ownership is refused without changing
  the schema-13 database or the artifact.
- Durable `pending` metadata exists before file creation. An exact current-owner
  file finalizes `ready` only after its recorded device/inode matches the open
  descriptor.
- The exact post-create/pre-ready crash-window file finalizes successfully; a
  pending symlink or mismatched file refuses.
- A busy holder prevents a second lease. Killing that holder releases only the
  kernel lock; restart acquires the same recorded inode.
- Ready-state missing, symlinked, replaced, and unlink-recreated artifacts all
  refuse while the durable metadata remains `ready`; none is recreated.
- The acquired lock descriptor is CLOEXEC and does not survive a child `exec`.
  Every temporary artifact is removed and the ordinary tmux fingerprint is
  unchanged.

## Consequence

The selected filesystem lifecycle is viable on this host for the isolated
schema-14 primitive. The production state migration can use the existing
private-file and nonblocking `Flock` discipline, while retaining the new lock
as a stable artifact rather than treating it like D16's removable cutover
lease.

## Limits

- The database is a deliberately minimal SQLite model, not WSNav's Rust schema
  migration, HostId row, marker, onboarding journal, or CompoundOperation.
- It does not race real materialization, presentation loss, broker/helper
  claims, provider preparation, OpenCode `POST /session`, or provider exec.
- It observes the local filesystem only. Network filesystems, host reset, and
  arbitrary external state-root mutation remain outside this probe.

## Status

**Narrow schema-14 stable-lock primitive validated; D17.0 integration and
recovery races remain required.**

[fixture]: ../../../spikes/fixtures/d17-provisional-lock.json
