# D23 provider-native stop and contextual visibility acceptance

Date: 2026-09-03

Status: D23, developed from `54cf0db`, is locally accepted and byte-identically
installed for operator inspection. This record claims no remote CI or live
Codex/OpenCode lifecycle interaction.

## Accepted boundary

- WSNav exposes no Navigator `p` action, Park footer/help/marker, controller
  action, public `park` command, or public lifecycle-action export. `Enter`
  remains the single attach/start/resume/recover control, and contextual `n`
  still creates a separate Workstream.
- Exiting the native provider TUI is the ordinary stop-and-keep-visible path.
  A stopped or schema-15 internally parked Workstream remains visible and
  resumes through `Enter` without a distinct parked marker.
- Archive is Workstreams-only. It revalidates and stops the exact owned
  provider process/private Runtime before hiding the Workstream. Missing,
  changed, or ambiguous identity refuses without hiding or signalling the
  provider.
- Terminal onboarding recovery uses the same compound Archive path: exact stop
  commits first, the matching journal is resolved under `provisional.lock`,
  and only then is the Workstream hidden. Any incomplete phase remains visible
  and retryable.
- Restore is Archived-only. It clears archive visibility without launching or
  attaching and atomically normalizes only internal `parked` to `open` while
  preserving the stopped Runtime and provider binding. Archived
  `recovery_required` state remains recovery-required.
- Workstreams and Archived have separate footer and floating-Help surfaces, so
  Archive and Restore are never advertised together. The compact Workstreams
  footer says `n new`; the full-inner-width, fully bordered Help panel says
  `Enter open` and `Esc back`, omits the old self-closing reminder and footer,
  and accepts `Esc` or `q` while open.
- Schema 15, private-tmux ownership, provider-native history/output, attention,
  naming, and native branching remain unchanged.

## Repository evidence

The final `scripts/check` passed on Rust 1.98.0 and tmux 3.7c. It ran strict
formatting and Clippy, 385 library tests, 10 presentation integration tests,
locked packaging, dependency advisory/license/source policy, shell and Python
checks, fixture validation, current-source and CLI acceptance, 43 focused
presentation tests, 34 focused current-state tests, and Markdown-link
validation over 57 files. After this acceptance record and its index link were
added, the dedicated Markdown-link validator passed over the resulting 58
files; current-source acceptance and staged/unstaged diff checks also passed.

The first complete-gate attempt stopped at strict Clippy because the new
restore regression contained 114 lines against the 100-line limit. Luna
factored its setup into a disposable fixture and split recovery preservation
into a second focused test; the final complete gate then passed. No behavioral
boundary was weakened and no lint allowance was added.

Deterministic tests additionally prove that:

- Archive terminates an exact disposable provider process and removes its
  private Runtime socket before committing archive visibility;
- a mismatched process-birth identity leaves the provider, socket, Workstream
  lifecycle, revision, and visibility untouched;
- the actual managed Archive route resolves a terminal onboarding journal,
  archives the stopped Workstream, then restores it to `open`/stopped without
  re-fencing or relaunching it;
- Restore preserves Runtime and provider-binding records and does not weaken an
  archived `recovery_required` Workstream; and
- public CLI parsing, Navigator dispatch, footer, help, and card markers omit
  Park while retaining page-local Archive/Restore and `n`/`Enter` behavior;
  and
- the Help panel has full-inner-width geometry and all four borders, with `?`
  inert and either `Esc` or `q` returning to the current page.

## Installed-artifact evidence

Before the final UI-refinement replacement, the installed `wsnav 0.1.0`
executable hash was:

```text
978905447c2fc7a7c80ad4eea72e03d78cf1f14b0169e5f9db51b105b1b02ea3  ~/.local/bin/wsnav
```

The locked release was built and atomically installed to
`~/.local/bin/wsnav`. The release and installed executable are both mode 0755,
the same size, and byte-identical:

```text
08023657e5b7c81eb48bf5e3cee7d5741f52b1d9c63f74a37a567563c1994191  target/release/wsnav
08023657e5b7c81eb48bf5e3cee7d5741f52b1d9c63f74a37a567563c1994191  ~/.local/bin/wsnav
```

The installed binary reports `wsnav 0.1.0`, opens the existing schema-15 root,
reports no unresolved operations, lists Archive and Restore in public help,
and omits Park.

## Unclaimed evidence

- No provider input, native provider exit, live Archive/Restore, observer
  installation, trust change, or ordinary tmux server was exercised.
- No remote CI, alternate Rust/tmux compatibility run, or real-provider
  lifecycle acceptance is claimed.
- The internal schema-15 `parked` value, exact-stop implementation, and legacy
  historical evidence remain; this checkpoint removes their ordinary product
  surface rather than migrating or rewriting stored state.
