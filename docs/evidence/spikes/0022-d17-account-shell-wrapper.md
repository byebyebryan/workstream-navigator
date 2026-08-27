# Spike 0022: D17 account-shell wrapper

## Question

Can a controlled D17 account-shell wrapper preserve observable interactive
non-login startup state while sourcing the operator's Bash/Zsh user RC exactly
once, then replace only `codex` and `opencode` aliases/functions with the
broker-owned functions?

## Procedure and isolation

The deterministic harness is
[`spikes/d17-account-shell-wrapper.py`](../../../spikes/d17-account-shell-wrapper.py),
with its sanitized [fixture][fixture]. It creates mode-`0700` temporary homes,
Zsh directories, wrapper files, and private tmux servers. It starts no provider
and leaves the normal home, `ZDOTDIR`, shell configuration, and ordinary tmux
server untouched.

Each controlled user RC sets a prompt, option, environment marker, ordinary
alias/function, conflicting `codex` alias, conflicting `opencode` function,
and one private count marker. The normal non-login shell and wrapper shell are
run independently. The wrapper sources the original user RC, removes only the
two conflicting provider definitions, and installs placeholder controlled
functions. A bounded probe retains only enum/boolean observations after the
temporary root is deleted.

The harness also exercises a user-RC abort and a login invocation. It does not
run a real provider command.

## Result

The fixture passed on Bash `5.3.15`, Zsh `5.9.2`, and tmux `3.7c`.

- Both wrappers source the controlled user RC exactly once, retain the user
  marker, prompt readiness, `noclobber`, ordinary alias, ordinary function, and
  original `HOME`; Zsh also restores the original `ZDOTDIR` before user RC
  execution.
- Both wrappers replace only the conflicting `codex` alias and `opencode`
  function with controlled functions after startup.
- A user-RC abort leaves the controlled functions uninstalled and returns a
  bounded startup-abort result.
- Zsh's wrapper sees login mode and refuses it. Bash has the important opposite
  behavior: interactive login Bash does not load the supplied `--rcfile`, so
  the wrapper never executes. D17 must reject requested login mode in its
  launcher before starting Bash; a later nested login shell is an unmanaged
  bypass.
- Every private server and temporary artifact cleaned up, and the ordinary tmux
  fingerprint was unchanged.

## Consequence

The controlled-wrapper approach is viable for the tested non-login startup
shape, but the login policy belongs at launch preflight rather than solely in a
Bash wrapper. This is a D17 implementation requirement, not an optional UX
check.

## Limits

- The study controls the user RC and compares the installed system startup
  behavior only as it exists on this host. It does not prove arbitrary future
  system RC behavior, wrapper-file replacement detection, all options/functions
  a real operator might define, or account-wide startup compatibility.
- It proves only placeholder function installation. Provider grammar, broker
  I/O, signal handling, Runtime ownership, and native provider effects remain
  separate D17.0 gates.

## Status

**Non-login wrapper candidate validated; Bash login preflight is mandatory.**

[fixture]: ../../../spikes/fixtures/d17-account-shell-wrapper.json
