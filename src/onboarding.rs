//! Pure D17 onboarding preparation boundaries.
#![allow(
    dead_code,
    reason = "the hidden D17 broker consumes this pure boundary only at the atomic cutover; keeping it dormant preserves D16 behavior"
)]
//!
//! This module does not create state, a shell, or a provider process. It
//! reduces the bounded argv observed by the future account-shell function to
//! either an explicitly unmanaged provider command or a normalized fresh-TUI
//! launch artifact. The broker will persist only the artifact's digest.

use std::ffi::OsString;

use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{
    domain::ProviderKind,
    provider::d17_grammar::{Classification, classify},
};

const ARGUMENT_DIGEST_VERSION: &str = "d17-fresh-argv-v1";

/// A shell command classification that is safe to make before any state or
/// provider effect.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ShellCommandDecision {
    ManagedFresh(FreshProviderLaunch),
    ExplicitlyUnmanaged,
}

/// The normalized provider argv bound into a future one-shot capability.
///
/// This is transient broker input. Callers must persist only
/// [`Self::argv_digest`], never the arguments themselves.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct FreshProviderLaunch {
    provider: ProviderKind,
    arguments: Vec<String>,
    argv_digest: String,
}

impl FreshProviderLaunch {
    #[must_use]
    pub(crate) const fn provider(&self) -> ProviderKind {
        self.provider
    }

    #[must_use]
    pub(crate) fn arguments(&self) -> &[String] {
        &self.arguments
    }

    /// Returns a versioned digest of the exact normalized command that the
    /// hidden helper must revalidate before provider exec.
    #[must_use]
    pub(crate) fn argv_digest(&self) -> &str {
        &self.argv_digest
    }

    /// Reconstructs a direct native invocation without a shell command
    /// string. This stays private to the future helper boundary.
    #[must_use]
    pub(crate) fn native_program(&self) -> Vec<OsString> {
        std::iter::once(OsString::from(self.provider.as_str()))
            .chain(self.arguments.iter().map(OsString::from))
            .collect()
    }
}

/// Reduces a shell's bounded provider argv to the pinned fresh-TUI grammar.
///
/// Exact information/auth commands are deliberately returned as unmanaged so
/// the shell can execute provider-owned behavior without a `WSNav` reservation.
/// Every other unrecognized, non-UTF-8, or unsafe shape refuses before state
/// or a provider effect.
pub(crate) fn classify_shell_command(
    provider: ProviderKind,
    arguments: &[OsString],
) -> Result<ShellCommandDecision, OnboardingCommandError> {
    let arguments = arguments
        .iter()
        .map(|argument| {
            argument
                .to_str()
                .map(str::to_owned)
                .ok_or(OnboardingCommandError::NonUtf8Argument)
        })
        .collect::<Result<Vec<_>, _>>()?;
    match classify(provider, &arguments).map_err(|_| OnboardingCommandError::UnpromotableCommand)? {
        Classification::ManagedFresh(arguments) => {
            Ok(ShellCommandDecision::ManagedFresh(FreshProviderLaunch {
                provider,
                argv_digest: digest_arguments(provider, &arguments),
                arguments,
            }))
        }
        Classification::ExplicitlyUnmanaged => Ok(ShellCommandDecision::ExplicitlyUnmanaged),
    }
}

fn digest_arguments(provider: ProviderKind, arguments: &[String]) -> String {
    let mut digest = Sha256::new();
    digest.update(ARGUMENT_DIGEST_VERSION.as_bytes());
    digest.update([0]);
    digest.update(provider.as_str().as_bytes());
    for argument in arguments {
        digest.update([0]);
        digest.update(argument.as_bytes());
    }
    format!("{ARGUMENT_DIGEST_VERSION}:{}", hex(&digest.finalize()))
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    bytes.iter().fold(
        String::with_capacity(bytes.len() * 2),
        |mut output, byte| {
            let _ = write!(output, "{byte:02x}");
            output
        },
    )
}

/// A bounded refusal which intentionally contains no shell argument values.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub(crate) enum OnboardingCommandError {
    #[error("provider argument is not UTF-8")]
    NonUtf8Argument,
    #[error("provider command is not a promotable fresh-TUI invocation")]
    UnpromotableCommand,
}

#[cfg(test)]
mod tests {
    use super::{FreshProviderLaunch, ShellCommandDecision, classify_shell_command};
    use crate::domain::ProviderKind;

    fn arguments(values: &[&str]) -> Vec<std::ffi::OsString> {
        values.iter().map(std::ffi::OsString::from).collect()
    }

    fn managed(provider: ProviderKind, values: &[&str]) -> FreshProviderLaunch {
        match classify_shell_command(provider, &arguments(values)).unwrap() {
            ShellCommandDecision::ManagedFresh(launch) => launch,
            ShellCommandDecision::ExplicitlyUnmanaged => panic!("expected managed command"),
        }
    }

    #[test]
    fn managed_command_normalizes_and_binds_only_a_digest() {
        let launch = managed(
            ProviderKind::OpenCode,
            &["-m", "openai/gpt-5.6", "--agent", "build", "--mini"],
        );
        assert_eq!(launch.provider(), ProviderKind::OpenCode);
        assert_eq!(
            launch.arguments(),
            ["--model", "openai/gpt-5.6", "--agent", "build", "--mini"]
        );
        assert!(launch.argv_digest().starts_with("d17-fresh-argv-v1:"));
        assert_eq!(
            launch.native_program(),
            arguments(&[
                "opencode",
                "--model",
                "openai/gpt-5.6",
                "--agent",
                "build",
                "--mini"
            ])
        );
    }

    #[test]
    fn equivalent_short_and_long_forms_have_one_launch_digest() {
        let short = managed(ProviderKind::Codex, &["-m", "gpt-5.6"]);
        let long = managed(ProviderKind::Codex, &["--model", "gpt-5.6"]);
        assert_eq!(short.arguments(), long.arguments());
        assert_eq!(short.argv_digest(), long.argv_digest());
        assert_ne!(
            short.argv_digest(),
            managed(ProviderKind::OpenCode, &["--model", "gpt-5.6"]).argv_digest()
        );
    }

    #[test]
    fn information_and_auth_shapes_are_explicitly_unmanaged() {
        for (provider, values) in [
            (ProviderKind::Codex, &["login"][..]),
            (ProviderKind::Codex, &["--help"][..]),
            (ProviderKind::OpenCode, &["providers"][..]),
            (ProviderKind::OpenCode, &["--version"][..]),
        ] {
            assert_eq!(
                classify_shell_command(provider, &arguments(values)),
                Ok(ShellCommandDecision::ExplicitlyUnmanaged)
            );
        }
    }

    #[test]
    fn session_path_prompt_and_secret_shapes_refuse_before_any_effect() {
        for (provider, values) in [
            (ProviderKind::Codex, &["resume", "--last"][..]),
            (ProviderKind::Codex, &["--cd", "other"][..]),
            (ProviderKind::OpenCode, &["--session", "known"][..]),
            (ProviderKind::OpenCode, &["--prompt", "initial"][..]),
            (ProviderKind::OpenCode, &["--model", "sk-secret"][..]),
        ] {
            assert!(classify_shell_command(provider, &arguments(values)).is_err());
        }
    }
}
