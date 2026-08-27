//! D17 fresh-TUI grammar contract.
//!
//! This mirrors the pinned 0.150.0/1.18.23 study in typed Rust and feeds the
//! dormant broker command boundary. It does not intercept or launch a provider.
#![allow(
    dead_code,
    reason = "the pure D17 command boundary stays dormant until atomic cutover so D16 command behavior remains unchanged"
)]

use std::collections::BTreeSet;

use crate::domain::ProviderKind;

const MAX_ARGUMENTS: usize = 16;
const MAX_ARGUMENT_BYTES: usize = 160;
const MAX_REPLAY_LIMIT: u16 = 10_000;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Classification {
    ManagedFresh(Vec<String>),
    ExplicitlyUnmanaged,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum GrammarError {
    UnsupportedShape,
    UnsafeArgument,
    DuplicateOption,
    InvalidValue,
}

pub(crate) fn classify(
    provider: ProviderKind,
    arguments: &[String],
) -> Result<Classification, GrammarError> {
    validate_shape(arguments)?;
    match provider {
        ProviderKind::Codex => classify_codex(arguments),
        ProviderKind::OpenCode => classify_opencode(arguments),
    }
}

fn validate_shape(arguments: &[String]) -> Result<(), GrammarError> {
    if arguments.len() > MAX_ARGUMENTS {
        return Err(GrammarError::UnsafeArgument);
    }
    for argument in arguments {
        if argument.is_empty()
            || argument.len() > MAX_ARGUMENT_BYTES
            || argument == "--"
            || argument.contains('=')
            || argument.chars().any(char::is_whitespace)
            || argument.chars().any(char::is_control)
        {
            return Err(GrammarError::UnsafeArgument);
        }
    }
    Ok(())
}

fn safe_value(value: &str, allow_slash: bool) -> Result<(), GrammarError> {
    let permitted = |character: char| {
        character.is_ascii_alphanumeric()
            || matches!(character, '.' | '_' | ':' | '-')
            || (allow_slash && character == '/')
    };
    let lowercased = value.to_ascii_lowercase();
    if value.is_empty()
        || value.len() > MAX_ARGUMENT_BYTES
        || !value.chars().all(permitted)
        || ["sk-", "sk_", "token-", "bearer-", "ghp_", "xoxb-"]
            .iter()
            .any(|prefix| lowercased.starts_with(prefix))
    {
        return Err(GrammarError::InvalidValue);
    }
    Ok(())
}

fn classify_codex(arguments: &[String]) -> Result<Classification, GrammarError> {
    if matches!(arguments, [value] if matches!(value.as_str(), "-h" | "--help" | "-V" | "--version" | "login"))
    {
        return Ok(Classification::ExplicitlyUnmanaged);
    }
    let mut normalized = Vec::new();
    let mut seen = BTreeSet::new();
    let mut index = 0;
    while index < arguments.len() {
        match arguments[index].as_str() {
            "--oss" | "--search" | "--no-alt-screen" | "--approve-for-me" => {
                let flag = &arguments[index];
                if !seen.insert(flag.clone()) {
                    return Err(GrammarError::DuplicateOption);
                }
                normalized.push(flag.clone());
                index += 1;
            }
            "-m" | "--model" | "--local-provider" | "-s" | "--sandbox" | "-a"
            | "--ask-for-approval" => {
                let canonical = match arguments[index].as_str() {
                    "-m" | "--model" => "--model",
                    "--local-provider" => "--local-provider",
                    "-s" | "--sandbox" => "--sandbox",
                    "-a" | "--ask-for-approval" => "--ask-for-approval",
                    _ => unreachable!("matched option must have a canonical form"),
                };
                let value = arguments
                    .get(index + 1)
                    .ok_or(GrammarError::UnsupportedShape)?;
                if !seen.insert(canonical.to_owned()) {
                    return Err(GrammarError::DuplicateOption);
                }
                safe_value(value, true)?;
                if (canonical == "--local-provider"
                    && !matches!(value.as_str(), "lmstudio" | "ollama"))
                    || (canonical == "--sandbox"
                        && !matches!(
                            value.as_str(),
                            "read-only" | "workspace-write" | "danger-full-access"
                        ))
                    || (canonical == "--ask-for-approval"
                        && !matches!(value.as_str(), "on-request" | "never"))
                {
                    return Err(GrammarError::InvalidValue);
                }
                normalized.extend([canonical.to_owned(), value.clone()]);
                index += 2;
            }
            _ => return Err(GrammarError::UnsupportedShape),
        }
    }
    if seen.contains("--local-provider") && !seen.contains("--oss") {
        return Err(GrammarError::InvalidValue);
    }
    Ok(Classification::ManagedFresh(normalized))
}

fn classify_opencode(arguments: &[String]) -> Result<Classification, GrammarError> {
    if matches!(arguments, [value] if matches!(value.as_str(), "-h" | "--help" | "-v" | "--version" | "providers"))
    {
        return Ok(Classification::ExplicitlyUnmanaged);
    }
    let mut normalized = Vec::new();
    let mut seen = BTreeSet::new();
    let mut index = 0;
    while index < arguments.len() {
        match arguments[index].as_str() {
            "--pure" | "--auto" | "--mini" | "--no-replay" => {
                let flag = &arguments[index];
                if !seen.insert(flag.clone()) {
                    return Err(GrammarError::DuplicateOption);
                }
                normalized.push(flag.clone());
                index += 1;
            }
            "-m" | "--model" | "--agent" | "--replay-limit" => {
                let canonical = match arguments[index].as_str() {
                    "-m" | "--model" => "--model",
                    "--agent" => "--agent",
                    "--replay-limit" => "--replay-limit",
                    _ => unreachable!("matched option must have a canonical form"),
                };
                let value = arguments
                    .get(index + 1)
                    .ok_or(GrammarError::UnsupportedShape)?;
                if !seen.insert(canonical.to_owned()) {
                    return Err(GrammarError::DuplicateOption);
                }
                if canonical == "--replay-limit" {
                    if !value.bytes().all(|byte| byte.is_ascii_digit())
                        || value
                            .parse::<u16>()
                            .map_or(true, |limit| limit > MAX_REPLAY_LIMIT)
                    {
                        return Err(GrammarError::InvalidValue);
                    }
                } else {
                    safe_value(value, canonical == "--model")?;
                }
                normalized.extend([canonical.to_owned(), value.clone()]);
                index += 2;
            }
            _ => return Err(GrammarError::UnsupportedShape),
        }
    }
    Ok(Classification::ManagedFresh(normalized))
}

#[cfg(test)]
mod tests {
    use crate::domain::ProviderKind;

    use super::{Classification, GrammarError, classify};

    fn arguments(values: &[&str]) -> Vec<String> {
        values.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn codex_normalizes_only_the_pinned_fresh_tui_options() {
        assert_eq!(
            classify(
                ProviderKind::Codex,
                &arguments(&["-m", "gpt-5.6", "--oss", "--local-provider", "ollama"]),
            ),
            Ok(Classification::ManagedFresh(arguments(&[
                "--model",
                "gpt-5.6",
                "--oss",
                "--local-provider",
                "ollama"
            ])))
        );
        assert_eq!(
            classify(
                ProviderKind::Codex,
                &arguments(&["--local-provider", "ollama"])
            ),
            Err(GrammarError::InvalidValue)
        );
    }

    #[test]
    fn codex_session_path_profile_prompt_and_secret_like_forms_refuse() {
        for values in [
            &["resume", "--last"][..],
            &["--remote", "ws://127.0.0.1:8080"],
            &["--profile", "other"],
            &["--cd", "elsewhere"],
            &["prompt"][..],
            &["--model", "sk-secret"],
        ] {
            assert!(classify(ProviderKind::Codex, &arguments(values)).is_err());
        }
    }

    #[test]
    fn opencode_normalizes_only_the_pinned_fresh_tui_options() {
        assert_eq!(
            classify(
                ProviderKind::OpenCode,
                &arguments(&["-m", "openai/gpt-5.6", "--agent", "build", "--mini"]),
            ),
            Ok(Classification::ManagedFresh(arguments(&[
                "--model",
                "openai/gpt-5.6",
                "--agent",
                "build",
                "--mini"
            ])))
        );
        for values in [
            &["project-path"][..],
            &["--session", "existing"],
            &["--port", "4096"],
            &["--prompt", "initial"],
            &["--replay-limit", "10001"],
        ] {
            assert!(classify(ProviderKind::OpenCode, &arguments(values)).is_err());
        }
    }

    #[test]
    fn exact_information_and_auth_shapes_remain_unmanaged() {
        for (provider, values) in [
            (ProviderKind::Codex, &["--help"][..]),
            (ProviderKind::Codex, &["login"][..]),
            (ProviderKind::OpenCode, &["--version"][..]),
            (ProviderKind::OpenCode, &["providers"][..]),
        ] {
            assert_eq!(
                classify(provider, &arguments(values)),
                Ok(Classification::ExplicitlyUnmanaged)
            );
        }
    }
}
