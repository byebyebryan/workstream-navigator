# D16 host-local simplification acceptance

Status: Complete on 2026-08-20. The full disposable repository gate and the
explicitly authorized live local and ordinary-SSH-entered-host gate passed.

## Operator and privacy boundary

The live run used each machine's ordinary WSNav state and installed binary.
It did not create an SSH transport inside WSNav, copy a Workstream between
hosts, or capture provider or terminal content. Evidence was limited to
schema/table counts and hashes, executable hashes, opaque Runtime identity,
process birth and liveness, private-socket device/inode/mode, tmux pane roles
and client counts, command exit status, and cleanup assertions. No prompt,
response, completed output, scrollback, terminal frame, credential, provider
payload, repository path, or raw SSH stream was retained.

The SSH-entered host had one exact live Codex Runtime and no live OpenCode
Runtime. Live OpenCode standby handover was therefore not exercised; its
bounded restart/acknowledgement paths remain covered by the disposable D16
cutover suites.

## Candidate and repository gate

Both machines ran tmux 3.7c. The accepted executable SHA-256 was
`17284f7f1b054564be3843ee4a457defd5302e996779f0be673190f103bb8462`.
After acceptance, that exact executable was installed atomically as the
canonical `wsnav` on both machines.

The final `scripts/check` run passed formatting, Clippy, 304 library tests,
all integration suites, package build and verification, Cargo Deny advisory,
ban, license, and source policy, shell/Python/fixture checks, the D12
presentation harness, D16 retired-source/CLI acceptance, and staged plus
unstaged whitespace checks. The complete test result was 399 passing tests
with one controlled D15 timing study ignored. The five focused D16 integration
suites contributed 82 passing tests.

### Post-acceptance source correction

Later on 2026-08-20, operator visual feedback found that the replacement D16
renderer had added a redundant page banner and had not carried forward the
established semantic colors or activity ages. The current source removes that
banner, projects the already-authoritative `last_activity_at_millis` through
the bounded passive application snapshot, restores the Project, provider,
lifecycle, attention, age, key, border, and background-only selection styles,
and allocates Project accents over the actual rendered viewport. A subsequent
operator card refinement makes Project headers name-only, removes repeated host
labels, places provider and right-aligned age together, gives the second line to
the lifecycle marker and native thread name, and drops the synthetic
`Workstream` prefix from the stable short-ID fallback. Deterministic
terminal-buffer tests cover those semantics, terminal-cell-width truncation,
and reclaimed mouse geometry. A further regression pass makes bounded status
and guidance prose wrap with matching list/mouse geometry, gives Parked
lifecycle precedence over sticky attention markers, permits a Running
attachment to be replaced during ordinary A-to-B switching, and safely revives
only an exact owned dead provider-helper pane after Park. Operator testing of
that installed correction then exposed a resume bootstrap cycle: the controller
waited for native SessionStart to move an exact Runtime out of `Starting` before
attaching the terminal client that may be needed for SessionStart itself. The
current source attaches once the exact owned Runtime exists, including while it
is `Starting`, while retaining refusal for `Stopped`, `Unknown`, missing, or
identity-changed evidence. Operator testing of the installed correction then
found the adjacent optimistic-revision race: native lifecycle acceptance could
advance the same Runtime and Workstream revisions after the navigator snapshot
but before attachment preflight. The current source permits one fresh passive
snapshot and one retry only when both opaque IDs remain exact and the Runtime
remains attachable; a second revision change or identity rotation still
refuses. A later Cubey Park/reopen attempt exposed an adjacent durable
convergence gap: an explicit second Park could change an exact absent Runtime
from `unknown` to `stopped` while leaving its Workstream
`recovery_required`, after which neither Start nor Recover had valid authority.
Park now atomically resolves that exact case to `parked/stopped` while retaining
the provider binding and sticky attention. That lifecycle-correction tree's
`scripts/check` gate passed 411 tests with the same one controlled D15 timing
study ignored; the five focused D16 integration suites contribute 89 passing
tests. The controlled D15 private-tmux study was also run separately and passed
with attachment against exact Runtime records that remained `Starting`.

The complete 411-test correction was installed locally for operator inspection
as executable SHA-256
`7117a7731bf83d1545e129755730bedbfec42d02c9e4c3b586f6472d252de300`. The
affected Cubey Workstream was repaired only through the public exact-ID Park and
Start actions: it converged from `recovery_required/stopped` to
`parked/stopped`, then reused its retained Codex session in a new exact private
Runtime generation. Content-free proof confirmed the recorded provider PID as
the live pane and process-group/session leader at the Cubey cwd; no terminal
content or provider payload was inspected. This operational correction was not
part of the earlier live acceptance recorded below. The executable hash and
399/82 counts above remain the exact evidence for that accepted candidate.
These corrections change navigator rendering/controller, exact
presentation-helper replacement, and the existing Park state transition; they
add no schema or provider interaction contract. This note does not represent a
second live acceptance.

A later source-only compact-navigator correction flattens one-Location Projects
instead of repeating their Project and label-source names, retains a minimal
tree only for multi-Location Projects, packs footer hints as whole key/action
pairs, and replaces generically wrapped help prose with a concise colored
key/action grid. The resulting current-tree `scripts/check` gate passed 413
tests with the same controlled D15 study ignored; the five focused D16 suites
contribute 91 passing tests. This later UI-only source was atomically installed
locally for operator inspection as executable SHA-256
`bcf48bc69d392f0bdea36845eb480038451bb5e4fb7837d09f08ddbad2438c47`; it was
not live-accepted as part of the evidence above.

## SSH-entered-host acceptance

The first confirmed schema-12-to-13 cutover completed its durable transition
and preserved the exact live Runtime, but post-cutover presentation creation
failed closed. Content-free diagnosis found two tmux integration assumptions:

- a POSIX SSH locale causes tmux to sanitize literal tab format delimiters;
  every private presentation and legacy-proof tmux command now uses tmux's
  native UTF-8 mode; and
- tmux changes an attached owner-only socket from `0600` to `0700`; socket
  proof now accepts only that mode transition on the same device and inode,
  while still rejecting replacement or group/world access.

The same review also corrected normal `q` shutdown: the in-pane navigator
detaches presentation clients, then the outer owner observes its exit, stops
the private presentation server, and removes the proven artifacts. A pane no
longer attempts to kill and clean its own controlling server.

After those corrections and the complete repository gate:

1. The schema-13 registry opened through the direct host-local facade.
2. The presentation opened, detached, and reconnected with the same private
   presentation socket inode and exact navigator/provider roles.
3. The outer SSH disconnect and reconnect left the live Runtime on the same
   private socket inode, Runtime generation, native session, Codex PID, and
   process birth. No Start, Resume, Park, or Runtime rotation occurred during
   that proof.
4. A content-discarding native attach created one client on that exact Runtime
   while the same Codex pane PID remained live, then detached normally.
5. Only after continuity and reattachment passed was the acceptance Runtime
   deliberately parked for cleanup. Its process and private socket then
   disappeared, and all three recorded Runtimes were stopped.

The host retained one HostIdentity, one Codex integration, three
ProjectLocations, three Workstreams, three Runtimes, two provider bindings,
two attention rows, and the existing zero operation/handle/request inventory.
Schema 13 derived three same-host Projects. Legacy client files, transition
lease, handover journal, activation acknowledgement, presentation artifacts,
and the staged acceptance executable were absent afterward. Observer
ownership and native trust remained `Ready`.

## Local acceptance

The local registry entered the confirmation path with host schema 12 and all
13 Runtimes stopped. Before confirmation, the run recorded deterministic row
counts and hashes for every authoritative schema-12 table. Exact interactive
`yes` confirmation was then supplied through the ordinary navigator entrypoint.

The migration produced host schema 13, nine same-host Projects from 11
ProjectLocations, and retained all 13 Workstreams and Runtimes. Every table
whose schema was unchanged retained its exact pre-cutover row hash. The only
expected structural hash changes were `host_identity.schema_version` from 12
to 13 and the new derived `project_locations.project_id` relationship; every
Location joined one of the nine derived Projects and the foreign-key check was
empty. The pre-existing inventory remained one HostIdentity, one Codex
integration, 11 ProjectLocations, 13 Workstreams, 13 stopped Runtimes, 11
provider bindings, 11 attention rows, two compound operations, zero OpenCode
Runtime handles, and two independent-creation requests.

The new presentation opened and detached successfully, then its normal `q`
path removed the exact presentation artifacts. Legacy client files and all
transition/handover artifacts were absent. Observer ownership and native trust
remained `Ready`.

## Cleanup and result

Every disposable diagnostic state root was removed. Both ordinary state roots
are current schema 13 with no live acceptance presentation. The SSH acceptance
Runtime was parked only after the continuity gate and is stopped; the local
inventory was already fully stopped. No default tmux server was targeted, no
provider session was copied or deleted, and no provider content entered the
evidence.

D16's full exit gate is complete. D0-D15 remote-control records remain
historical evidence; current multi-host operation is ordinary SSH followed by
one host-local WSNav instance per host.
