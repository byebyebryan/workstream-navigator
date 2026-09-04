# D24 Archived Catalog and WSNav-Owned Forget

Status: locally accepted from the current working tree and installed
byte-identically for operator inspection. This record does not claim remote CI
or live-provider operator acceptance and does not transfer D23 evidence.

## Contract exercised

D24 keeps Archived as a secondary Workstream catalog. Archived `Enter` uses the
ordinary exact attach/start/resume/recover path while retaining `archived_at`;
provider exit still stops an explicitly opened Runtime. `u` restores only
visibility and preserves a live Runtime. `x` opens a distinct Forget
confirmation, and `wsnav forget <workstream-id> <revision>` is its public
revision-fenced equivalent.

Native attachment completion is reconciled separately from the tmux client's
own exit status. An exact still-live provider is an ordinary detach and remains
unchanged. One retained dead pane is accepted as normal provider exit only
when its PID and launch cwd match the Runtime record, its process is absent,
the exact topology remains stable, and its exit status is zero. WSNav then
removes that private server and records stopped/parked while retaining
`archived_at`. Non-zero or ambiguous evidence remains untouched and
unavailable. Attachment preflight repeats the same proof so a clean exit left
by an older helper is reconciled and returned through the same stopped surface
path instead of becoming a stale-open error.
The private Runtime installs and reconciles an exact server-local `pane-died`
hook that detaches its client on provider exit while `remain-on-exit` retains
the dead pane as evidence. This lets attachment return immediately so the
classification can run; the hook itself makes no lifecycle decision.
After an exact stopped outcome, the outer presentation helper emits only
terminal reset, display-clear, cursor-home, and cursor-show controls before
waiting; no WSNav prose is written into the provider pane. Normal detach and
ambiguous/non-zero exit preserve the display. Stopped and internally parked
cards use a static gray `■` that remains readable against the selection
highlight, while live/idle cards remain unmarked.

Forget first validates the exact archived row, revision, onboarding state, and
WSNav-owned operation graph. A live owned Runtime is exact-stopped before one
schema-15 transaction deletes only the selected Workstream and its WSNav-owned
rows: dependent OpenCode settled messages/handles, provider binding, Runtime,
attention, selected-workstream creation metadata, and target-owned completed
operations plus execution-target metadata. Nullable child source lineage is
severed. Stale, ambiguous, unresolved, or shared operation effects retain the
Workstream and roll back. Provider-native history, Project/Location/Git/files,
and unrelated records are not touched.

## Validation

Commands were run from the checkout with disposable test state:

```text
cargo check                                                        exit 0
cargo test --lib navigator::view::tests                       exit 0
cargo test --lib navigator::controller::tests                  exit 0
cargo test --lib actions::tests                                 exit 0
cargo test --lib attachment_end_                                exit 0
cargo test --lib attachment_preflight_self_heals_a_retained_normal_exit exit 0
cargo test --lib retained_dead_pane                             exit 0
cargo test --lib provider_exit_hook_detaches_the_client_and_retains_dead_evidence exit 0
cargo test --lib stopped_provider_surface_emits_only_terminal_reset_and_clear_controls exit 0
cargo test --lib state::current_state_tests                    exit 0
cargo test --lib app::tests                                     exit 0
cargo test --lib state::current_state_tests::forgetting_archived_workstream_removes_only_its_owned_graph exit 0
git diff --check                                                   exit 0
scripts/check                                                      exit 0
cargo build --locked --release                                     exit 0
```

The focused graph test proves selected-row deletion, dependent WSNav-row
deletion, child-lineage severing, preservation of the Project and both
ProjectLocations, stale revision refusal, and unresolved operation refusal.
The state suite also proves visibility-only Restore leaves a live Runtime
byte-for-byte unchanged. Navigator tests prove Archived Enter/Restore/Forget
intent, stable footer rows, and page-local help and modal copy. The action and
CLI tests prove Archived start authorization, Forget of an exact-stopped
retained Runtime, and the public revision-fenced route.

The complete gate passed strict Clippy, 399 library tests, 10 presentation
integration tests, package verification, license/advisory policy, current
source and presentation acceptance, documentation links, and staged/unstaged
diff checks. `cargo-deny` reported only the already accepted duplicate-version
warnings; advisories, bans, licenses, and sources all passed.

The exit-classification tests use only disposable private tmux sockets. They
prove a live detach does not advance either revision, a zero-status retained
pane is exact-cleaned and parked while archived visibility is retained,
preflight repairs an older stranded clean exit, and a non-zero retained pane is
left unchanged. A live disposable tmux test also proves the `pane-died` hook
detaches the client while retaining the exact dead-pane status for
classification. A focused presentation test proves the stopped surface writes
only the fixed terminal control sequence, and the marker tests distinguish
stopped from live/idle while proving selected-card contrast. Sanitized
inspection of the operator-reported failure found the same exact
retained-dead-pane shape; no provider content was captured.

## Installed artifact

The locked release was atomically installed to `~/.local/bin/wsnav`. Source and
installed artifacts are mode `0755`, size 7,345,952 bytes, and byte-identical:

```text
4b81709179b308e32039aa53573b12c9a787b9249547fd5835cd6c10e85c9518  target/release/wsnav
4b81709179b308e32039aa53573b12c9a787b9249547fd5835cd6c10e85c9518  ~/.local/bin/wsnav
```

The installed binary reports `wsnav 0.1.0`, and its generated help exposes the
revision-fenced public `forget` command.

## Not claimed here

- No remote CI or live-provider acceptance was run for this checkpoint.
- No provider-native history was archived or deleted.
- No transcript preview, provider-thread delete, bulk pruning, migration/reset,
  project cleanup, or page-change lifecycle behavior was added.
