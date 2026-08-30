//! Pure onboarding preparation boundaries.
//!
//! This module does not create state, a shell, or a provider process. It
//! reduces the bounded argv observed by the account-shell function to
//! either an explicitly unmanaged provider command or a normalized fresh-TUI
//! launch artifact. The broker persists only the artifact's digest.

use std::{
    ffi::OsString,
    path::{Component, Path, PathBuf},
};

use sha2::{Digest, Sha256};
use thiserror::Error;
use uuid::Uuid;

use crate::{
    domain::{IdGenerator, LocationId, OperationId, ProviderKind, Revision, RuntimeId},
    provider::grammar::{Classification, classify},
    runtime::RuntimePaths,
};

const ARGUMENT_DIGEST_VERSION: &str = "wsnav-fresh-argv-v1";
const CLAIM_DIGEST_VERSION: &str = "wsnav-launch-claims-v1";
const TOKEN_VERIFIER_VERSION: &str = "wsnav-launch-verifier-v1";
const BOOT_PROVENANCE_VERSION: &str = "wsnav-boot-v1";
const MAX_CAPABILITY_LIFETIME_MILLIS: i64 = 60_000;
const MAX_CAPABILITY_TEXT_BYTES: usize = 256;

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
    #[cfg(test)]
    #[must_use]
    pub(crate) fn native_program(&self) -> Vec<OsString> {
        std::iter::once(OsString::from(self.provider.as_str()))
            .chain(self.arguments.iter().map(OsString::from))
            .collect()
    }
}

/// The complete in-memory claim set for one brokered launch capability.
///
/// Paths and provider arguments exist only while the broker and hidden helper
/// revalidate the launch. Durable state receives [`Self::digest`] and the
/// opaque references carried by the Runtime/operation graph, never these raw
/// values or the live capability token.
#[derive(Clone, Eq, PartialEq)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "The capability deliberately carries every independently revalidated authority claim."
)]
pub(crate) struct LaunchCapabilityClaims {
    operation_id: OperationId,
    presentation_id: Uuid,
    presentation_revision: Revision,
    slot_generation: Uuid,
    lease_generation: i64,
    candidate_runtime_id: RuntimeId,
    runtime_paths: RuntimePaths,
    provider: ProviderKind,
    shell_cwd: PathBuf,
    worktree_root: PathBuf,
    location_id: LocationId,
    runtime_generation: String,
    registry_generation: String,
    shell_pid: u32,
    shell_birth: String,
    shell_process_group: u32,
    shell_session: u32,
    argv_digest: String,
    boot_provenance: String,
}

impl std::fmt::Debug for LaunchCapabilityClaims {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LaunchCapabilityClaims")
            .field("operation_id", &"<opaque>")
            .field("presentation_id", &"<opaque>")
            .field("candidate_runtime_id", &"<opaque>")
            .field("provider", &self.provider)
            .field("runtime_paths", &"<private>")
            .field("shell_cwd", &"<private>")
            .field("worktree_root", &"<private>")
            .field("shell_identity", &"<private>")
            .finish_non_exhaustive()
    }
}

impl LaunchCapabilityClaims {
    /// Validates and retains an exact capability claim set before the broker
    /// creates a durable reservation.
    #[allow(
        clippy::too_many_arguments,
        reason = "the launch boundary must receive each independently bound claim explicitly"
    )]
    pub(crate) fn new(
        operation_id: OperationId,
        presentation_id: Uuid,
        presentation_revision: Revision,
        slot_generation: Uuid,
        lease_generation: i64,
        candidate_runtime_id: RuntimeId,
        runtime_paths: RuntimePaths,
        provider: ProviderKind,
        shell_cwd: PathBuf,
        worktree_root: PathBuf,
        location_id: LocationId,
        runtime_generation: String,
        registry_generation: String,
        shell_pid: u32,
        shell_birth: String,
        shell_process_group: u32,
        shell_session: u32,
        argv_digest: String,
        boot_provenance: String,
    ) -> Result<Self, CapabilityError> {
        if lease_generation <= 0
            || shell_pid == 0
            || shell_process_group == 0
            || shell_session == 0
            || !is_normalized_absolute_path(&runtime_paths.directory)
            || !is_normalized_absolute_path(&runtime_paths.socket)
            || !is_normalized_absolute_path(&runtime_paths.config)
            || !is_normalized_absolute_path(&shell_cwd)
            || !is_normalized_absolute_path(&worktree_root)
            || !is_bounded_text(&runtime_paths.session_name)
            || !is_bounded_text(&runtime_generation)
            || !is_bounded_text(&registry_generation)
            || !is_bounded_text(&shell_birth)
            || !is_versioned_sha256(&argv_digest, ARGUMENT_DIGEST_VERSION)
            || !is_versioned_sha256(&boot_provenance, BOOT_PROVENANCE_VERSION)
        {
            return Err(CapabilityError::InvalidClaims);
        }
        Ok(Self {
            operation_id,
            presentation_id,
            presentation_revision,
            slot_generation,
            lease_generation,
            candidate_runtime_id,
            runtime_paths,
            provider,
            shell_cwd,
            worktree_root,
            location_id,
            runtime_generation,
            registry_generation,
            shell_pid,
            shell_birth,
            shell_process_group,
            shell_session,
            argv_digest,
            boot_provenance,
        })
    }

    /// Returns the only durable representation of the complete claim set.
    #[must_use]
    pub(crate) fn digest(&self) -> String {
        let mut digest = Sha256::new();
        digest.update(CLAIM_DIGEST_VERSION.as_bytes());
        update_claim(&mut digest, "operation", &self.operation_id.to_string());
        update_claim(
            &mut digest,
            "presentation",
            &self.presentation_id.to_string(),
        );
        update_claim(
            &mut digest,
            "presentation_revision",
            &self.presentation_revision.value().to_string(),
        );
        update_claim(
            &mut digest,
            "slot_generation",
            &self.slot_generation.to_string(),
        );
        update_claim(
            &mut digest,
            "lease_generation",
            &self.lease_generation.to_string(),
        );
        update_claim(
            &mut digest,
            "candidate_runtime",
            &self.candidate_runtime_id.to_string(),
        );
        update_claim_path(
            &mut digest,
            "runtime_directory",
            &self.runtime_paths.directory,
        );
        update_claim_path(&mut digest, "runtime_socket", &self.runtime_paths.socket);
        update_claim_path(&mut digest, "runtime_config", &self.runtime_paths.config);
        update_claim(
            &mut digest,
            "runtime_session",
            &self.runtime_paths.session_name,
        );
        update_claim(&mut digest, "provider", self.provider.as_str());
        update_claim_path(&mut digest, "shell_cwd", &self.shell_cwd);
        update_claim_path(&mut digest, "worktree_root", &self.worktree_root);
        update_claim(&mut digest, "location", &self.location_id.to_string());
        update_claim(&mut digest, "runtime_generation", &self.runtime_generation);
        update_claim(
            &mut digest,
            "registry_generation",
            &self.registry_generation,
        );
        update_claim(&mut digest, "shell_pid", &self.shell_pid.to_string());
        update_claim(&mut digest, "shell_birth", &self.shell_birth);
        update_claim(
            &mut digest,
            "shell_process_group",
            &self.shell_process_group.to_string(),
        );
        update_claim(
            &mut digest,
            "shell_session",
            &self.shell_session.to_string(),
        );
        update_claim(&mut digest, "argv_digest", &self.argv_digest);
        update_claim(&mut digest, "boot_provenance", &self.boot_provenance);
        format!("{CLAIM_DIGEST_VERSION}:sha256:{}", hex(&digest.finalize()))
    }
}

/// Persistable verifier and bounded references for a one-shot capability.
/// The live token is intentionally absent.
#[derive(Clone, Eq, PartialEq)]
pub(crate) struct LaunchCapabilityMetadata {
    token_id: String,
    verifier: String,
    expiry_monotonic_millis: i64,
    claims_digest: String,
}

impl std::fmt::Debug for LaunchCapabilityMetadata {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LaunchCapabilityMetadata")
            .field("token_id", &"<opaque>")
            .field("verifier", &"<redacted>")
            .field("expiry_monotonic_millis", &self.expiry_monotonic_millis)
            .field("claims_digest", &"<digest>")
            .finish()
    }
}

impl LaunchCapabilityMetadata {
    /// Reconstructs the bounded metadata that the broker persisted for a
    /// helper-side verification. The live capability token remains absent.
    pub(crate) fn from_persisted(
        token_id: String,
        verifier: String,
        expiry_monotonic_millis: i64,
        claims_digest: String,
    ) -> Result<Self, CapabilityError> {
        if Uuid::parse_str(&token_id).is_err()
            || expiry_monotonic_millis <= 0
            || !is_versioned_sha256(&verifier, TOKEN_VERIFIER_VERSION)
            || !is_versioned_sha256(&claims_digest, CLAIM_DIGEST_VERSION)
        {
            return Err(CapabilityError::InvalidToken);
        }
        Ok(Self {
            token_id,
            verifier,
            expiry_monotonic_millis,
            claims_digest,
        })
    }

    #[must_use]
    pub(crate) fn token_id(&self) -> &str {
        &self.token_id
    }

    #[must_use]
    pub(crate) fn verifier(&self) -> &str {
        &self.verifier
    }

    #[must_use]
    pub(crate) const fn expiry_monotonic_millis(&self) -> i64 {
        self.expiry_monotonic_millis
    }

    #[must_use]
    pub(crate) fn claims_digest(&self) -> &str {
        &self.claims_digest
    }
}

/// A live capability returned only through the broker's private channel.
/// Its `Debug` form deliberately never reveals the token.
pub(crate) struct LaunchCapability {
    token: String,
    metadata: LaunchCapabilityMetadata,
}

impl std::fmt::Debug for LaunchCapability {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LaunchCapability")
            .field("metadata", &self.metadata)
            .finish_non_exhaustive()
    }
}

impl LaunchCapability {
    /// Issues a short-lived, verifier-backed capability from a validated
    /// claim set. The injected identity source keeps all tests deterministic.
    pub(crate) fn issue(
        claims: &LaunchCapabilityClaims,
        now_monotonic_millis: i64,
        expiry_monotonic_millis: i64,
        id_generator: &dyn IdGenerator,
    ) -> Result<Self, CapabilityError> {
        let lifetime = expiry_monotonic_millis
            .checked_sub(now_monotonic_millis)
            .ok_or(CapabilityError::InvalidExpiry)?;
        if now_monotonic_millis < 0 || lifetime <= 0 || lifetime > MAX_CAPABILITY_LIFETIME_MILLIS {
            return Err(CapabilityError::InvalidExpiry);
        }
        let token_id = id_generator.uuid();
        let secret = format!(
            "{}{}",
            id_generator.uuid().simple(),
            id_generator.uuid().simple()
        );
        let token_id_text = token_id.to_string();
        let token = format!("{token_id_text}.{secret}");
        let metadata = LaunchCapabilityMetadata {
            token_id: token_id_text,
            verifier: verifier(&token),
            expiry_monotonic_millis,
            claims_digest: claims.digest(),
        };
        Ok(Self { token, metadata })
    }

    /// Returns the private-channel token. The caller must not log, persist,
    /// serialize, or render it.
    #[must_use]
    pub(crate) fn token(&self) -> &str {
        &self.token
    }

    #[must_use]
    pub(crate) fn metadata(&self) -> &LaunchCapabilityMetadata {
        &self.metadata
    }
}

/// Revalidates a live token against persisted metadata and the helper's newly
/// observed claim set. State code must atomically record consumption only
/// after this pure check succeeds under `provisional.lock`.
pub(crate) fn verify_launch_capability(
    token: &str,
    metadata: &LaunchCapabilityMetadata,
    claims: &LaunchCapabilityClaims,
    now_monotonic_millis: i64,
) -> Result<(), CapabilityError> {
    if now_monotonic_millis < 0 || now_monotonic_millis >= metadata.expiry_monotonic_millis {
        return Err(CapabilityError::Expired);
    }
    if Uuid::parse_str(&metadata.token_id).is_err()
        || !is_versioned_sha256(&metadata.verifier, TOKEN_VERIFIER_VERSION)
        || !is_versioned_sha256(&metadata.claims_digest, CLAIM_DIGEST_VERSION)
    {
        return Err(CapabilityError::InvalidToken);
    }
    let (token_id, _) = token.split_once('.').ok_or(CapabilityError::InvalidToken)?;
    if !valid_launch_capability_token(token)
        || token_id != metadata.token_id
        || !constant_time_eq(verifier(token).as_bytes(), metadata.verifier.as_bytes())
    {
        return Err(CapabilityError::InvalidToken);
    }
    if !constant_time_eq(
        claims.digest().as_bytes(),
        metadata.claims_digest.as_bytes(),
    ) {
        return Err(CapabilityError::ClaimMismatch);
    }
    Ok(())
}

/// Validates the exact private-channel grammar emitted by
/// [`LaunchCapability::issue`]. Keeping this next to the issuer prevents CLI
/// transport preflight from drifting away from the helper's token grammar.
pub(crate) fn valid_launch_capability_token(token: &str) -> bool {
    let Some((token_id, secret)) = token.split_once('.') else {
        return false;
    };
    let Ok(parsed_token_id) = Uuid::parse_str(token_id) else {
        return false;
    };
    parsed_token_id.to_string() == token_id
        && secret.len() == 64
        && secret.bytes().all(is_lower_hex)
}

fn verifier(token: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(TOKEN_VERIFIER_VERSION.as_bytes());
    digest.update([0]);
    digest.update(token.as_bytes());
    format!(
        "{TOKEN_VERIFIER_VERSION}:sha256:{}",
        hex(&digest.finalize())
    )
}

fn update_claim(digest: &mut Sha256, name: &str, value: &str) {
    digest.update([0]);
    digest.update(name.as_bytes());
    digest.update([0]);
    digest.update(u64::try_from(value.len()).unwrap_or(u64::MAX).to_be_bytes());
    digest.update(value.as_bytes());
}

fn update_claim_path(digest: &mut Sha256, name: &str, path: &Path) {
    update_claim(digest, name, &path.to_string_lossy());
}

fn is_normalized_absolute_path(path: &Path) -> bool {
    path.is_absolute()
        && path.to_str().is_some()
        && path
            .components()
            .all(|component| matches!(component, Component::RootDir | Component::Normal(_)))
}

fn is_bounded_text(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_CAPABILITY_TEXT_BYTES
        && !value.chars().any(char::is_control)
}

fn is_versioned_sha256(value: &str, version: &str) -> bool {
    value
        .strip_prefix(&format!("{version}:sha256:"))
        .is_some_and(|digest| digest.len() == 64 && digest.bytes().all(is_lower_hex))
}

const fn is_lower_hex(byte: u8) -> bool {
    byte.is_ascii_digit() || matches!(byte, b'a'..=b'f')
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
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
    format!(
        "{ARGUMENT_DIGEST_VERSION}:sha256:{}",
        hex(&digest.finalize())
    )
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

/// Bounded failures for capability issuance and helper-side revalidation.
/// Values, paths, and tokens intentionally never cross this error boundary.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub(crate) enum CapabilityError {
    #[error("launch capability claims are invalid")]
    InvalidClaims,
    #[error("launch capability expiry is invalid")]
    InvalidExpiry,
    #[error("launch capability token is invalid")]
    InvalidToken,
    #[error("launch capability has expired")]
    Expired,
    #[error("launch capability claims do not match")]
    ClaimMismatch,
}

#[cfg(test)]
mod tests {
    use std::{
        path::{Path, PathBuf},
        sync::atomic::{AtomicU64, Ordering},
    };

    use super::{
        CapabilityError, FreshProviderLaunch, LaunchCapability, LaunchCapabilityClaims,
        ShellCommandDecision, classify_shell_command, valid_launch_capability_token,
        verify_launch_capability,
    };
    use crate::{
        domain::{IdGenerator, LocationId, OperationId, ProviderKind, Revision, RuntimeId},
        runtime::RuntimePaths,
    };
    use uuid::Uuid;

    #[derive(Default)]
    struct SequenceIds(AtomicU64);

    impl IdGenerator for SequenceIds {
        fn uuid(&self) -> Uuid {
            Uuid::from_u128(u128::from(self.0.fetch_add(1, Ordering::Relaxed) + 1))
        }
    }

    fn arguments(values: &[&str]) -> Vec<std::ffi::OsString> {
        values.iter().map(std::ffi::OsString::from).collect()
    }

    fn managed(provider: ProviderKind, values: &[&str]) -> FreshProviderLaunch {
        match classify_shell_command(provider, &arguments(values)).unwrap() {
            ShellCommandDecision::ManagedFresh(launch) => launch,
            ShellCommandDecision::ExplicitlyUnmanaged => panic!("expected managed command"),
        }
    }

    fn claims(provider: ProviderKind, shell_birth: &str) -> LaunchCapabilityClaims {
        let state_root = Path::new("/tmp/wsnav-current-state");
        LaunchCapabilityClaims::new(
            OperationId::from(Uuid::from_u128(11)),
            Uuid::from_u128(12),
            Revision::INITIAL,
            Uuid::from_u128(13),
            7,
            RuntimeId::from(Uuid::from_u128(14)),
            RuntimePaths::for_runtime(state_root, RuntimeId::from(Uuid::from_u128(14))),
            provider,
            PathBuf::from("/tmp/wsnav-current-state/worktree/nested"),
            PathBuf::from("/tmp/wsnav-current-state/worktree"),
            LocationId::from(Uuid::from_u128(15)),
            "runtime-generation-16".to_owned(),
            "registry-generation-17".to_owned(),
            101,
            shell_birth.to_owned(),
            101,
            101,
            managed(provider, &["--model", "gpt-5.6"])
                .argv_digest()
                .to_owned(),
            format!("wsnav-boot-v1:sha256:{}", "c".repeat(64)),
        )
        .unwrap()
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
        assert!(launch.argv_digest().starts_with("wsnav-fresh-argv-v1:"));
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

    #[test]
    fn launch_capability_persists_only_verifier_and_revalidates_every_claim_digest() {
        let capability_claims = claims(ProviderKind::Codex, "birth-a");
        let ids = SequenceIds::default();
        let capability = LaunchCapability::issue(&capability_claims, 10, 1_010, &ids).unwrap();
        let metadata = capability.metadata().clone();
        assert!(valid_launch_capability_token(capability.token()));
        assert_ne!(metadata.token_id(), capability.token());
        assert!(!format!("{capability:?}").contains(capability.token()));
        assert!(
            metadata
                .verifier()
                .starts_with("wsnav-launch-verifier-v1:sha256:")
        );
        assert!(
            metadata
                .claims_digest()
                .starts_with("wsnav-launch-claims-v1:sha256:")
        );
        assert_eq!(metadata.expiry_monotonic_millis(), 1_010);
        verify_launch_capability(capability.token(), &metadata, &capability_claims, 11).unwrap();

        for invalid in [
            "00000000000000000000000000000001.0000000000000000000000000000000000000000000000000000000000000000",
            "00000000-0000-0000-0000-000000000001.0000",
            "00000000-0000-0000-0000-000000000001.000000000000000000000000000000000000000000000000000000000000000G",
            "00000000-0000-0000-0000-000000000001.0000000000000000000000000000000000000000000000000000000000000000.extra",
        ] {
            assert!(!valid_launch_capability_token(invalid));
        }

        let different_provider = claims(ProviderKind::OpenCode, "birth-a");
        assert_eq!(
            verify_launch_capability(capability.token(), &metadata, &different_provider, 11),
            Err(CapabilityError::ClaimMismatch)
        );
        let different_shell = claims(ProviderKind::Codex, "birth-b");
        assert_eq!(
            verify_launch_capability(capability.token(), &metadata, &different_shell, 11),
            Err(CapabilityError::ClaimMismatch)
        );
    }

    #[test]
    fn launch_capability_refuses_expiry_invalid_tokens_and_invalid_lifetimes() {
        let capability_claims = claims(ProviderKind::Codex, "birth-a");
        let ids = SequenceIds::default();
        assert!(matches!(
            LaunchCapability::issue(&capability_claims, 10, 10, &ids),
            Err(CapabilityError::InvalidExpiry)
        ));
        assert!(matches!(
            LaunchCapability::issue(&capability_claims, 10, 60_011, &ids),
            Err(CapabilityError::InvalidExpiry)
        ));
        let capability = LaunchCapability::issue(&capability_claims, 10, 1_010, &ids).unwrap();
        let metadata = capability.metadata().clone();
        assert_eq!(
            verify_launch_capability(capability.token(), &metadata, &capability_claims, 1_010),
            Err(CapabilityError::Expired)
        );
        let mut wrong_token = capability.token().to_owned();
        let last = wrong_token.pop().unwrap();
        wrong_token.push(if last == '0' { '1' } else { '0' });
        assert_eq!(
            verify_launch_capability(&wrong_token, &metadata, &capability_claims, 11),
            Err(CapabilityError::InvalidToken)
        );
    }

    #[test]
    fn launch_capability_digest_binds_every_authority_category() {
        let claims = claims(ProviderKind::Codex, "birth-a");
        let digest = claims.digest();

        let mut changed = claims.clone();
        changed.operation_id = OperationId::from(Uuid::from_u128(98));
        assert_ne!(changed.digest(), digest);
        changed = claims.clone();
        changed.presentation_id = Uuid::from_u128(99);
        assert_ne!(changed.digest(), digest);
        changed = claims.clone();
        changed.presentation_revision = Revision::try_from(2).unwrap();
        assert_ne!(changed.digest(), digest);
        changed = claims.clone();
        changed.slot_generation = Uuid::from_u128(99);
        assert_ne!(changed.digest(), digest);
        changed = claims.clone();
        changed.lease_generation = 8;
        assert_ne!(changed.digest(), digest);
        changed = claims.clone();
        changed.candidate_runtime_id = RuntimeId::from(Uuid::from_u128(100));
        assert_ne!(changed.digest(), digest);
        changed = claims.clone();
        changed.runtime_paths.directory = PathBuf::from("/tmp/wsnav-current-state/other-runtime");
        assert_ne!(changed.digest(), digest);
        changed = claims.clone();
        changed.runtime_paths.socket =
            PathBuf::from("/tmp/wsnav-current-state/other-runtime/socket");
        assert_ne!(changed.digest(), digest);
        changed = claims.clone();
        changed.runtime_paths.config =
            PathBuf::from("/tmp/wsnav-current-state/other-runtime/tmux.conf");
        assert_ne!(changed.digest(), digest);
        changed = claims.clone();
        changed.runtime_paths.session_name = "wsnav-other".to_owned();
        assert_ne!(changed.digest(), digest);
        changed = claims.clone();
        changed.provider = ProviderKind::OpenCode;
        assert_ne!(changed.digest(), digest);
        changed = claims.clone();
        changed.shell_cwd = PathBuf::from("/tmp/wsnav-current-state/worktree/other");
        assert_ne!(changed.digest(), digest);
        changed = claims.clone();
        changed.worktree_root = PathBuf::from("/tmp/wsnav-current-state/other-worktree");
        assert_ne!(changed.digest(), digest);
        changed = claims.clone();
        changed.location_id = LocationId::from(Uuid::from_u128(101));
        assert_ne!(changed.digest(), digest);
        changed = claims.clone();
        changed.runtime_generation = "runtime-generation-other".to_owned();
        assert_ne!(changed.digest(), digest);
        changed = claims.clone();
        changed.registry_generation = "registry-generation-other".to_owned();
        assert_ne!(changed.digest(), digest);
        changed = claims.clone();
        changed.shell_pid = 102;
        assert_ne!(changed.digest(), digest);
        changed = claims.clone();
        changed.shell_birth = "birth-other".to_owned();
        assert_ne!(changed.digest(), digest);
        changed = claims.clone();
        changed.shell_process_group = 102;
        assert_ne!(changed.digest(), digest);
        changed = claims.clone();
        changed.shell_session = 102;
        assert_ne!(changed.digest(), digest);
        changed = claims.clone();
        changed.argv_digest = managed(ProviderKind::Codex, &["--model", "gpt-5.7"])
            .argv_digest()
            .to_owned();
        assert_ne!(changed.digest(), digest);
        changed = claims.clone();
        changed.boot_provenance = format!("wsnav-boot-v1:sha256:{}", "d".repeat(64));
        assert_ne!(changed.digest(), digest);
    }
}
