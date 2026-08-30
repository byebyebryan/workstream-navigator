# D18 current-only consolidation acceptance

Date: 2026-08-29

Status: complete in the checkpoint commit containing this record. Repository
checks, the authorized destructive reset, direct ordinary schema-15 bootstrap,
installed parity, an 80-column shell-first launch, Rust 1.88 and Ubuntu 24.04
clean-host matrices, explicit native observer trust, disposable Codex/OpenCode
lifecycle acceptance, and complete cleanup pass.

## Candidate boundary

- Fresh state directly creates schema 15 with application ID `0x57534e56`.
- Schemas 12 through 14 and retired transition artifacts are inert refusal
  evidence; no migration, import, adoption, or compatibility route remains.
- Current state, presentation, Navigator, onboarding, provider grammar, and
  reconciliation code use semantic modules and hidden role names.
- Projects browsing and arbitrary registration are absent. The provisional
  account shell remains the path and provider-selection surface.
- The installed binary and release build are byte-identical at SHA-256
  `f732e2b16344b038cd05996501ce77be42302f7403de9720d156dbf24777d124`.

## Repository evidence

The final local repository gate passes formatting, strict Clippy, all retained tests,
packaging, dependency policy, shell/Python/fixture checks, generated-help and
semantic-source acceptance, disposable presentation/state acceptance, and
staged/unstaged diff checks.

A disposable Ubuntu 24.04 container copied the read-only source mount into
container-local storage, installed Rust 1.88.0, tmux 3.4, Bash, and Zsh, and ran
`cargo test --locked --all-targets --all-features`. The final corrected source
passed 350 library tests and 7 presentation integration tests. A separate
Rust 1.88/Debian/tmux 3.3a run passed the same matrix.

That clean-environment pass first exposed a tmux 3.4 startup race hidden by the
newer development tmux: a detached 80-column window could compress the
Navigator to one column while creating the preferred 96-column provider pane.
The candidate now starts detached presentations at the exact 129-by-24
two-pane geometry before splitting, with a unit command-contract test and the
disposable integration matrix passing.

The first ordinary 80-column attach then exposed a second, narrower race: the
installed tmux width hook reached the correct final 32/47 layout, but the
Navigator could inspect the transient attach topology and retain a stale error
banner. The controller now retries only that exact topology/resize operation
for at most 100 milliseconds. Two deterministic tests prove transient recovery
and persistent fail-closed behavior. The final local gate passes 350 library
tests and 7 presentation integration tests, and the repeated real 80-column
launch has no banner.

## Clean-break correction

[Spike 0027](../spikes/0027-d18-root-move-falsification.md) proves that an
unprivileged online release tool cannot establish the required race-free
zero-arbitrary-holder boundary around a coherent online backup. `/proc` may be
incomplete and a point-in-time scan cannot prevent a new cwd, descriptor,
executable, or socket holder before rename.

That result rejects the stronger backup-and-rollback contract; it does not
require compatibility machinery for discarded WSNav state. D18 now uses an
explicit destructive reset: stop exact owned processes and private servers,
remove the exact D17.1 observer declaration while schema 14 remains available,
atomically quarantine the whole root as non-input discarded state, install the
accepted D18 artifact, and directly bootstrap schema 15. No migration, import,
adoption, or symmetric state rollback is permitted.

## Ordinary reset and installation evidence

The operator explicitly authorized discarding the D17.1 WSNav state. Read-only
preflight found two exact live Codex Runtimes and no presentation. Both Runtime
records matched their provider process and private tmux identity, and the
installed D17.1 `park` path stopped them before reset. Provider-native Codex
history remained outside the WSNav root.

The installed D17.1 `remove-observer` path then removed the exact owned
declaration completely. No provider-settings remainder remained. With the
presentation and Runtime directories empty and no exact WSNav-owned process or
private server live, the complete schema-14 root was atomically renamed to a
sibling quarantine as discarded data. D18 does not read that directory.

The byte-identical D18 release artifact was atomically installed and directly
bootstrapped ordinary state with application ID `0x57534e56`, schema 15, and
zero Workstreams, Runtimes, or operations. Bootstrap, database, and provisional
lock artifacts are private. The corrected first-launch presentation is healthy,
detached, and ready for the operator's next `wsnav` attach.

After acceptance, the exact discarded D17.1 quarantine was deleted. That
deletion is not recoverable. Provider-native history remains provider-owned
outside the WSNav state root. Unrelated historical backup and test roots were
not changed.

## Disposable live-provider acceptance

The operator separately authorized real Codex and OpenCode launches in
disposable state roots, repositories, provider homes, XDG directories, and
private tmux servers. Codex authentication and OpenCode authentication were
copied only into their respective disposable provider homes with mode `0600`.
They were never printed, parsed, hashed, or placed in WSNav state. No prompt,
response, tool output, terminal frame, scrollback, transcript, credential,
native session identifier, PID, UUID, endpoint, or temporary path is retained
in this record.

The accepted candidate used Codex CLI `0.150.0`, OpenCode `1.18.23`, and tmux
`3.7c`. The installed executable remained byte-identical to the release build
at SHA-256
`f732e2b16344b038cd05996501ce77be42302f7403de9720d156dbf24777d124`.

### Codex

The native Codex review displayed the candidate observer declaration and the
operator explicitly trusted all four declared hooks. WSNav then promoted the
provisional shell to one exact managed Codex Runtime. A completed native turn
produced exact lifecycle and sticky-result-attention evidence. Selecting the
replacement Shell created no additional durable Workstream, Runtime, binding,
or operation.

Park stopped the exact provider process, observer participation, managed tmux
server, and Runtime socket while retaining the native binding. Ordinary Start
then launched a new Runtime generation against that exact retained session.
Codex `0.150.0` did not emit `SessionStart(source=resume)` merely by displaying
the resumed TUI; the Runtime therefore truthfully remained `starting`. The
next native turn emitted exact same-session prompt and settled evidence and
returned the Runtime to result attention. This is accepted for ordinary Start;
the separate recovery-required route still requires an exact
`SessionStart(source=resume)` and remains fail closed without it. A final Park
and presentation exit removed every disposable Codex process, private server,
observer declaration, profile, and Runtime artifact.

### OpenCode

WSNav promoted the provisional shell to one exact blank root OpenCode session,
started the native TUI, and established a generation-bound ready observer. A
completed native turn produced one exact settled-message identity and sticky
result attention. Selecting the replacement Shell again left all durable
managed-state counts unchanged.

Park removed the provider process, ready observer handle, managed tmux server,
and Runtime socket while retaining the exact binding. Ordinary Start then
created a new Runtime generation against the same native session with
`start_source=resume`, a corroborated native provider process, and a ready
generation-bound observer. Because an already-idle resumed OpenCode session
emitted no new lifecycle event, the Runtime correctly remained `starting`
until a future native event rather than synthesizing provider state. Final Park
and presentation exit removed every disposable OpenCode process, listener,
observer handle, private server, and Runtime artifact.

Two harness-only retries occurred before any provider effect: one disposable
state-root name exceeded the private tmux socket-path limit, and one isolated
XDG configuration initially exposed the account shell's native prompt setup.
Neither attempt created a Workstream, Runtime, provider binding, operation, or
provider process. Both were classified as pre-provider harness failures and
fully deleted before the accepted run.

## Cleanup and commit binding

Both accepted disposable roots, repositories, provider homes, authentication
copies, presentations, Runtime directories, helpers, observers, endpoints, and
private tmux servers were deleted after exact shutdown checks. The ordinary
WSNav state retained its pre-acceptance aggregate parked/stopped state and ready
Codex integration, with no presentation or Runtime server. The user's default
tmux server contained no WSNav session. The checkpoint commit containing this
record binds the accepted source and evidence to installed SHA-256
`f732e2b16344b038cd05996501ce77be42302f7403de9720d156dbf24777d124`.
