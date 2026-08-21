use super::{BTreeMap, OsString, Path, ProviderBinding};

/// Builds the only native provider command permitted for a managed Runtime.
#[must_use]
pub fn codex_launch_program(
    cwd: &Path,
    binding: Option<&ProviderBinding>,
) -> Vec<std::ffi::OsString> {
    let mut program = vec![
        "codex".into(),
        "--profile".into(),
        "wsnav-observer".into(),
        "-C".into(),
        cwd.as_os_str().to_owned(),
    ];
    if let Some(binding) = binding {
        program.push("resume".into());
        program.push(binding.native_session_id.native_id().to_owned().into());
    }
    program
}

/// Builds the recovery-only native Codex command. Deliberately omit a session
/// identifier when no authoritative binding survived: Codex then presents its
/// own resume picker, and only the observed `source=resume` selection may bind
/// the managed Runtime.
#[must_use]
pub fn codex_recovery_program(
    cwd: &Path,
    binding: Option<&ProviderBinding>,
) -> Vec<std::ffi::OsString> {
    let mut program = codex_launch_program(cwd, None);
    program.push("resume".into());
    if let Some(binding) = binding {
        program.push(binding.native_session_id.native_id().to_owned().into());
    }
    program
}

/// Builds the environment owned by a managed Codex Runtime.
///
/// A host-local launch can inherit a POSIX locale even when the terminal that
/// later attaches is UTF-8. Set the locale only for the owned Codex process
/// and its hook children, so its renderer has a stable UTF-8 contract without
/// changing the user's shell or an unmanaged provider session.
pub(super) fn managed_codex_environment() -> BTreeMap<OsString, OsString> {
    const UTF8_LOCALE: &str = "C.UTF-8";

    BTreeMap::from([
        ("LANG".into(), UTF8_LOCALE.into()),
        ("LC_CTYPE".into(), UTF8_LOCALE.into()),
        ("LC_ALL".into(), UTF8_LOCALE.into()),
    ])
}
