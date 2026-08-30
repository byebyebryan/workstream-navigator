//! controlled account-shell bootstrap.
//!
//! This module constructs only fixed private startup files and direct shell
//! argv for a provisional Runtime. It does not render a card, create durable
//! state, or invoke a provider itself.

use std::{
    collections::BTreeMap,
    env,
    ffi::OsString,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

use thiserror::Error;

use crate::{
    provisional::{
        HostMaterializationError, ProvisionalSlot,
        materialize_private_shell_with_startup_under_lease,
    },
    runtime::{
        NativeLaunch, PrivateRuntime, ProcessGroupProbe, RuntimeError, RuntimePaths, RuntimeStartup,
    },
    state::{CurrentState, ProvisionalLease},
};

const BASH_WRAPPER_FILE: &str = ".wsnav-provisional-bashrc";
const ZSH_WRAPPER_FILE: &str = ".zshrc";
const HOME_ENV: &str = "HOME";
const STATE_ROOT_ENV: &str = "WSNAV_STATE_ROOT";
const PRESENTATION_DIRECTORY_ENV: &str = "WSNAV_PRESENTATION_DIRECTORY";
const ORIGINAL_HOME_ENV: &str = "WSNAV_ORIGINAL_HOME";
const ORIGINAL_ZDOTDIR_ENV: &str = "WSNAV_ORIGINAL_ZDOTDIR";
const EXECUTABLE_ENV: &str = "WSNAV_EXECUTABLE";

const BASH_WRAPPER: &str = r#"if shopt -q login_shell; then
    printf '%s\n' 'WSNav onboarding requires a non-login shell' >&2
    return 64
fi
if [[ -f "${WSNAV_ORIGINAL_HOME:?}/.bashrc" ]] && ! source "${WSNAV_ORIGINAL_HOME}/.bashrc"; then
    printf '%s\n' 'WSNav account-shell startup was refused' >&2
    return 64
fi
unalias codex opencode 2>/dev/null || true
unset -f codex opencode 2>/dev/null || true
codex() {
    local wsnav_capability wsnav_status wsnav_consent wsnav_setup_status
    while :; do
    wsnav_capability="$("${WSNAV_EXECUTABLE:?}" _shell_gate --provider codex --shell-leader-pid "$$" -- "$@")"
    wsnav_status=$?
    if [[ "$wsnav_status" -eq 0 && -n "$wsnav_capability" && ${#wsnav_capability} -le 512 && "$wsnav_capability" != *$'\n'* && "$wsnav_capability" != *$'\r'* ]]; then
        exec "${WSNAV_EXECUTABLE}" _launch_helper --capability "$wsnav_capability" --provider codex -- "$@"
        printf '%s\n' 'WSNav onboarding command is unavailable' >&2
        return 64
    fi
    if [[ "$wsnav_status" -eq 10 ]]; then
        command codex "$@"
        return
    fi
    if [[ "$wsnav_status" -eq 11 ]]; then
        printf '%s' 'WSNav Codex observer setup is required. Allow exact profile setup and native /hooks review? [y/N] ' >&2
        IFS= read -r wsnav_consent || wsnav_consent=''
        if [[ "$wsnav_consent" == [yY] || "$wsnav_consent" == [yY][eE][sS] ]]; then
            "${WSNAV_EXECUTABLE}" _observer_setup --shell-leader-pid "$$" --consent
            wsnav_setup_status=$?
            if [[ "$wsnav_setup_status" -eq 0 ]]; then
                continue
            fi
        else
            printf '%s\n' 'WSNav Codex observer setup was declined' >&2
        fi
    fi
    printf '%s\n' 'WSNav onboarding command is unavailable' >&2
    return 64
    done
}
opencode() {
    local wsnav_capability
    wsnav_capability="$("${WSNAV_EXECUTABLE:?}" _shell_gate --provider opencode --shell-leader-pid "$$" -- "$@")"
    local wsnav_status=$?
    if [[ "$wsnav_status" -eq 0 && -n "$wsnav_capability" && ${#wsnav_capability} -le 512 && "$wsnav_capability" != *$'\n'* && "$wsnav_capability" != *$'\r'* ]]; then
        exec "${WSNAV_EXECUTABLE}" _launch_helper --capability "$wsnav_capability" --provider opencode -- "$@"
        printf '%s\n' 'WSNav onboarding command is unavailable' >&2
        return 64
    fi
    if [[ "$wsnav_status" -eq 10 ]]; then
        command opencode "$@"
        return
    fi
    printf '%s\n' 'WSNav onboarding command is unavailable' >&2
    return 64
}
"#;

const ZSH_WRAPPER: &str = r#"if [[ -o login ]]; then
    print -r -- 'WSNav onboarding requires a non-login shell' >&2
    return 64
fi
export ZDOTDIR="${WSNAV_ORIGINAL_ZDOTDIR:?}"
if [[ -f "${ZDOTDIR}/.zshrc" ]] && ! source "${ZDOTDIR}/.zshrc"; then
    print -r -- 'WSNav account-shell startup was refused' >&2
    return 64
fi
if (( $+aliases[codex] )); then unalias codex || return 64; fi
if (( $+aliases[opencode] )); then unalias opencode || return 64; fi
if (( $+functions[codex] )); then unfunction codex || return 64; fi
if (( $+functions[opencode] )); then unfunction opencode || return 64; fi
codex() {
    local wsnav_capability wsnav_status wsnav_consent wsnav_setup_status
    while :; do
    wsnav_capability="$("${WSNAV_EXECUTABLE:?}" _shell_gate --provider codex --shell-leader-pid "$$" -- "$@")"
    wsnav_status=$?
    if [[ "$wsnav_status" -eq 0 && -n "$wsnav_capability" && ${#wsnav_capability} -le 512 && "$wsnav_capability" != *$'\n'* && "$wsnav_capability" != *$'\r'* ]]; then
        exec "${WSNAV_EXECUTABLE}" _launch_helper --capability "$wsnav_capability" --provider codex -- "$@"
        print -r -- 'WSNav onboarding command is unavailable' >&2
        return 64
    fi
    if [[ "$wsnav_status" -eq 10 ]]; then
        command codex "$@"
        return
    fi
    if [[ "$wsnav_status" -eq 11 ]]; then
        print -n -- 'WSNav Codex observer setup is required. Allow exact profile setup and native /hooks review? [y/N] ' >&2
        read -r wsnav_consent || wsnav_consent=''
        if [[ "$wsnav_consent" == [yY] || "$wsnav_consent" == [yY][eE][sS] ]]; then
            "${WSNAV_EXECUTABLE}" _observer_setup --shell-leader-pid "$$" --consent
            wsnav_setup_status=$?
            if [[ "$wsnav_setup_status" -eq 0 ]]; then
                continue
            fi
        else
            print -r -- 'WSNav Codex observer setup was declined' >&2
        fi
    fi
    print -r -- 'WSNav onboarding command is unavailable' >&2
    return 64
    done
}
opencode() {
    local wsnav_capability
    wsnav_capability="$("${WSNAV_EXECUTABLE:?}" _shell_gate --provider opencode --shell-leader-pid "$$" -- "$@")"
    local wsnav_status=$?
    if [[ "$wsnav_status" -eq 0 && -n "$wsnav_capability" && ${#wsnav_capability} -le 512 && "$wsnav_capability" != *$'\n'* && "$wsnav_capability" != *$'\r'* ]]; then
        exec "${WSNAV_EXECUTABLE}" _launch_helper --capability "$wsnav_capability" --provider opencode -- "$@"
        print -r -- 'WSNav onboarding command is unavailable' >&2
        return 64
    fi
    if [[ "$wsnav_status" -eq 10 ]]; then
        command opencode "$@"
        return
    fi
    print -r -- 'WSNav onboarding command is unavailable' >&2
    return 64
}
"#;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AccountShellKind {
    Bash,
    Zsh,
}

impl AccountShellKind {
    fn from_path(path: &Path) -> Result<(Self, PathBuf), AccountShellError> {
        let path = canonical_executable(path, AccountShellError::ShellUnavailable)?;
        let kind = match path.file_name().and_then(|name| name.to_str()) {
            Some("bash") => Self::Bash,
            Some("zsh") => Self::Zsh,
            _ => return Err(AccountShellError::UnsupportedShell),
        };
        Ok((kind, path))
    }

    const fn wrapper_file(self) -> &'static str {
        match self {
            Self::Bash => BASH_WRAPPER_FILE,
            Self::Zsh => ZSH_WRAPPER_FILE,
        }
    }

    const fn wrapper_body(self) -> &'static str {
        match self {
            Self::Bash => BASH_WRAPPER,
            Self::Zsh => ZSH_WRAPPER,
        }
    }
}

/// Non-authoritative discovery paths inherited by a provisional shell's hidden
/// children. Each child must reopen and revalidate the marker, Runtime, and
/// schema-15 lease; this context alone can never grant ownership.
#[derive(Clone)]
pub(crate) struct AccountShellContext {
    state_root: PathBuf,
    presentation_directory: PathBuf,
}

impl std::fmt::Debug for AccountShellContext {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AccountShellContext")
            .field("state_root", &"<private>")
            .field("presentation_directory", &"<private>")
            .finish()
    }
}

impl AccountShellContext {
    /// Binds the wrapper to an existing presentation directory contained by
    /// the exact state root. The values are discovery inputs only; every
    /// helper must treat changes as an authority mismatch.
    pub(crate) fn new(
        state_root: &Path,
        presentation_directory: &Path,
    ) -> Result<Self, AccountShellError> {
        let state_root = canonical_context_directory(state_root)?;
        let presentation_directory = canonical_context_directory(presentation_directory)?;
        if presentation_directory == state_root || !presentation_directory.starts_with(&state_root)
        {
            return Err(AccountShellError::ContextUnavailable);
        }
        Ok(Self {
            state_root,
            presentation_directory,
        })
    }

    /// Reconstructs the non-authoritative discovery context inherited by a
    /// private account shell child. Missing, non-UTF-8, empty, or control-byte
    /// values fail closed before a helper can inspect state.
    pub(crate) fn from_environment() -> Result<Self, AccountShellError> {
        Self::from_environment_values(
            env::var_os(STATE_ROOT_ENV),
            env::var_os(PRESENTATION_DIRECTORY_ENV),
        )
    }

    #[must_use]
    pub(crate) fn state_root(&self) -> &Path {
        &self.state_root
    }

    #[must_use]
    pub(crate) fn presentation_directory(&self) -> &Path {
        &self.presentation_directory
    }

    fn from_environment_values(
        state_root: Option<OsString>,
        presentation_directory: Option<OsString>,
    ) -> Result<Self, AccountShellError> {
        let state_root = environment_path(state_root)?;
        let presentation_directory = environment_path(presentation_directory)?;
        Self::new(&state_root, &presentation_directory)
    }
}

/// A fully fixed provisional account-shell launch and its private bootstrap.
/// Its debug representation deliberately omits filesystem paths and retained
/// environment values.
pub(crate) struct AccountShellLaunch {
    launch: NativeLaunch,
    bootstrap: AccountShellBootstrap,
}

impl std::fmt::Debug for AccountShellLaunch {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AccountShellLaunch")
            .field("shell", &self.bootstrap.kind)
            .finish_non_exhaustive()
    }
}

impl AccountShellLaunch {
    /// Builds only a non-login Bash/Zsh startup plan. The caller must provide
    /// exact new Runtime paths so the wrapper never lands in user state.
    pub(crate) fn new(
        context: &AccountShellContext,
        runtime_paths: &RuntimePaths,
        seed_cwd: &Path,
        shell: &Path,
        original_home: &Path,
        original_zdotdir: Option<&Path>,
        executable: &Path,
    ) -> Result<Self, AccountShellError> {
        let seed_cwd = canonical_directory(seed_cwd, AccountShellError::SeedCwdUnavailable)?;
        let original_home = canonical_directory(original_home, AccountShellError::HomeUnavailable)?;
        let (kind, shell) = AccountShellKind::from_path(shell)?;
        let executable =
            canonical_executable(executable, AccountShellError::ExecutableUnavailable)?;
        let original_zdotdir = match kind {
            AccountShellKind::Bash => None,
            AccountShellKind::Zsh => Some(canonical_directory(
                // Zsh uses HOME when ZDOTDIR is unset. Preserve that native
                // account-shell default before redirecting startup through the
                // private wrapper.
                original_zdotdir.unwrap_or(original_home.as_path()),
                AccountShellError::ZdotdirUnavailable,
            )?),
        };
        let wrapper = runtime_paths.directory.join(kind.wrapper_file());
        let mut environment = BTreeMap::new();
        environment.insert(
            OsString::from(STATE_ROOT_ENV),
            path_environment_value(&context.state_root)?,
        );
        environment.insert(
            OsString::from(PRESENTATION_DIRECTORY_ENV),
            path_environment_value(&context.presentation_directory)?,
        );
        environment.insert(
            OsString::from(HOME_ENV),
            path_environment_value(&original_home)?,
        );
        environment.insert(
            OsString::from(ORIGINAL_HOME_ENV),
            path_environment_value(&original_home)?,
        );
        environment.insert(
            OsString::from(EXECUTABLE_ENV),
            path_environment_value(&executable)?,
        );
        if let Some(zdotdir) = &original_zdotdir {
            environment.insert(
                OsString::from(ORIGINAL_ZDOTDIR_ENV),
                path_environment_value(zdotdir)?,
            );
            environment.insert(
                OsString::from("ZDOTDIR"),
                path_environment_value(&runtime_paths.directory)?,
            );
        }
        let program = match kind {
            AccountShellKind::Bash => vec![
                shell.into_os_string(),
                OsString::from("--noprofile"),
                OsString::from("--rcfile"),
                wrapper.into_os_string(),
                OsString::from("-i"),
            ],
            AccountShellKind::Zsh => vec![shell.into_os_string(), OsString::from("-i")],
        };
        Ok(Self {
            launch: NativeLaunch {
                cwd: seed_cwd,
                program,
                environment,
            },
            bootstrap: AccountShellBootstrap {
                kind,
                expected_paths: runtime_paths.clone(),
            },
        })
    }

    #[must_use]
    pub(crate) fn launch(&self) -> &NativeLaunch {
        &self.launch
    }

    #[must_use]
    pub(crate) fn bootstrap(&self) -> &AccountShellBootstrap {
        &self.bootstrap
    }

    /// Materializes this exact account shell through the marker-first
    /// provisional seam. The atomic startup boundary is the only caller;
    /// this method does not add a current launch route.
    pub(crate) fn materialize_under_lease(
        &self,
        state: &CurrentState,
        provisional_lease: &ProvisionalLease,
        presentation_directory: &Path,
        slot: &ProvisionalSlot,
        runtime: &PrivateRuntime<'_>,
        process_group_probe: &dyn ProcessGroupProbe,
    ) -> Result<ProvisionalSlot, HostMaterializationError> {
        materialize_private_shell_with_startup_under_lease(
            state,
            provisional_lease,
            presentation_directory,
            slot,
            runtime,
            self.launch(),
            self.bootstrap(),
            process_group_probe,
        )
    }
}

/// Fixed startup-file writer for [`AccountShellLaunch`].
pub(crate) struct AccountShellBootstrap {
    kind: AccountShellKind,
    expected_paths: RuntimePaths,
}

impl RuntimeStartup for AccountShellBootstrap {
    fn prepare(&self, paths: &RuntimePaths) -> Result<(), RuntimeError> {
        if paths != &self.expected_paths {
            return Err(RuntimeError::StartupUnavailable);
        }
        let wrapper = paths.directory.join(self.kind.wrapper_file());
        write_private_new(&wrapper, self.kind.wrapper_body())
            .map_err(|_| RuntimeError::StartupUnavailable)
    }
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub(crate) enum AccountShellError {
    #[error("the account shell is unavailable")]
    ShellUnavailable,
    #[error("the account shell supports only Bash and Zsh interactive shells")]
    UnsupportedShell,
    #[error("the presentation seed cwd is unavailable")]
    SeedCwdUnavailable,
    #[error("the account-shell context is unavailable")]
    ContextUnavailable,
    #[error("the original account home is unavailable")]
    HomeUnavailable,
    #[error("the original Zsh directory is unavailable")]
    ZdotdirUnavailable,
    #[error("the WSNav executable is unavailable")]
    ExecutableUnavailable,
    #[error("the account-shell environment is unsafe")]
    UnsafeEnvironment,
}

fn canonical_directory(
    path: &Path,
    error: AccountShellError,
) -> Result<PathBuf, AccountShellError> {
    let canonical = fs::canonicalize(path).map_err(|_| error.clone())?;
    if canonical.is_dir() {
        Ok(canonical)
    } else {
        Err(error)
    }
}

/// Context paths originate outside the process through the private shell
/// environment. Refuse symlinks before canonicalization so a later
/// presentation proof can compare the exact on-disk location rather than an
/// attacker-selected alias.
fn canonical_context_directory(path: &Path) -> Result<PathBuf, AccountShellError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| AccountShellError::ContextUnavailable)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(AccountShellError::ContextUnavailable);
    }
    canonical_directory(path, AccountShellError::ContextUnavailable)
}

fn canonical_executable(
    path: &Path,
    unavailable: AccountShellError,
) -> Result<PathBuf, AccountShellError> {
    let canonical = fs::canonicalize(path).map_err(|_| unavailable.clone())?;
    let metadata = fs::metadata(&canonical).map_err(|_| unavailable.clone())?;
    if !metadata.is_file() {
        return Err(unavailable);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return Err(unavailable);
        }
    }
    Ok(canonical)
}

fn path_environment_value(path: &Path) -> Result<OsString, AccountShellError> {
    let value = path.to_str().ok_or(AccountShellError::UnsafeEnvironment)?;
    if value.is_empty() || value.chars().any(char::is_control) {
        return Err(AccountShellError::UnsafeEnvironment);
    }
    Ok(OsString::from(value))
}

fn environment_path(value: Option<OsString>) -> Result<PathBuf, AccountShellError> {
    let value = value.ok_or(AccountShellError::ContextUnavailable)?;
    let value = value
        .to_str()
        .filter(|value| !value.is_empty() && !value.chars().any(char::is_control))
        .ok_or(AccountShellError::ContextUnavailable)?;
    Ok(PathBuf::from(value))
}

fn write_private_new(path: &Path, body: &str) -> Result<(), std::io::Error> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(nix::libc::O_CLOEXEC | nix::libc::O_NOFOLLOW);
    }
    let mut file = options.open(path)?;
    file.write_all(body.as_bytes())?;
    file.sync_all()
}

#[cfg(test)]
mod tests {
    use std::{
        cell::RefCell,
        env,
        ffi::OsString,
        fs,
        path::{Path, PathBuf},
        process::Command,
    };

    use super::{
        AccountShellContext, AccountShellError, AccountShellLaunch, BASH_WRAPPER,
        BASH_WRAPPER_FILE, EXECUTABLE_ENV, HOME_ENV, ORIGINAL_HOME_ENV, ORIGINAL_ZDOTDIR_ENV,
        PRESENTATION_DIRECTORY_ENV, STATE_ROOT_ENV, ZSH_WRAPPER, ZSH_WRAPPER_FILE,
    };
    use crate::{
        domain::{Revision, RuntimeId},
        provisional::{
            ProvisionalSlot, SlotGeneration, materialize_private_shell_with_startup, read_marker,
        },
        runtime::{
            PrivateRuntime, ProcessGroupInfo, ProcessGroupProbe, ProcessProbe, ProcessProbeError,
            RuntimeError, RuntimePaths, RuntimeStartup, TmuxClient, TmuxInvocation, TmuxResponse,
        },
    };
    use uuid::Uuid;

    fn make_directory(path: &Path) {
        fs::create_dir(path).unwrap();
    }

    fn account_context(root: &Path) -> AccountShellContext {
        let presentation = root.join("presentation");
        if !presentation.exists() {
            make_directory(&presentation);
        }
        AccountShellContext::new(root, &presentation).unwrap()
    }

    fn executable(path: &Path) {
        fs::write(path, b"#!/bin/sh\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = fs::metadata(path).unwrap().permissions();
            permissions.set_mode(0o700);
            fs::set_permissions(path, permissions).unwrap();
        }
    }

    fn environment_value(launch: &crate::runtime::NativeLaunch, key: &str) -> Option<OsString> {
        launch.environment.get(&OsString::from(key)).cloned()
    }

    fn system_shell(name: &str) -> std::path::PathBuf {
        env::split_paths(&env::var_os("PATH").unwrap())
            .map(|directory| directory.join(name))
            .find(|candidate| candidate.is_file())
            .and_then(|candidate| fs::canonicalize(candidate).ok())
            .expect("the -supported account shell must be installed for its contract test")
    }

    fn run_controlled_shell(shell_name: &str, gate_status: i32, expected_probe: &str) {
        let temporary = tempfile::tempdir().unwrap();
        let seed_cwd = temporary.path().join("seed");
        let original_home = temporary.path().join("home");
        let original_zdotdir = temporary.path().join("zdotdir");
        let bin = temporary.path().join("bin");
        make_directory(&seed_cwd);
        make_directory(&original_home);
        make_directory(&original_zdotdir);
        make_directory(&bin);
        let shell = system_shell(shell_name);
        let wsnav = bin.join("wsnav");
        let provider = bin.join("codex");
        executable(&wsnav);
        executable(&provider);
        fs::write(
            &wsnav,
            format!(
                "#!/bin/sh\nif [ \"$1\" = _shell_gate ]; then [ \"$2\" = --provider ] && [ \"$3\" = codex ] && [ \"$4\" = --shell-leader-pid ] && [ \"$6\" = -- ] || exit 64; [ {gate_status} -eq 0 ] && printf opaque-capability; exit {gate_status}; fi\nif [ \"$1\" = _launch_helper ]; then [ \"$2\" = --capability ] && [ \"$3\" = opaque-capability ] && [ \"$4\" = --provider ] && [ \"$5\" = codex ] && [ \"$6\" = -- ] || exit 64; printf '%s\\n' managed > \"${{WSNAV_PROBE_OUT:?}}\"; exit 0; fi\nexit 64\n"
            ),
        )
        .unwrap();
        fs::write(
            &provider,
            "#!/bin/sh\nprintf '%s\\n' unmanaged > \"${WSNAV_PROBE_OUT:?}\"\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for path in [&wsnav, &provider] {
                let mut permissions = fs::metadata(path).unwrap().permissions();
                permissions.set_mode(0o700);
                fs::set_permissions(path, permissions).unwrap();
            }
        }
        let count = temporary.path().join("count");
        let rc = match shell_name {
            "bash" => original_home.join(".bashrc"),
            "zsh" => original_zdotdir.join(".zshrc"),
            _ => unreachable!(),
        };
        fs::write(
            rc,
            "printf x >> \"${WSNAV_RC_COUNT:?}\"\nalias codex='false'\nopencode() { false; }\n",
        )
        .unwrap();
        let paths = RuntimePaths::for_runtime(temporary.path(), RuntimeId::new());
        let plan = AccountShellLaunch::new(
            &account_context(temporary.path()),
            &paths,
            &seed_cwd,
            &shell,
            &original_home,
            (shell_name == "zsh").then_some(original_zdotdir.as_path()),
            &wsnav,
        )
        .unwrap();
        fs::create_dir_all(&paths.directory).unwrap();
        plan.bootstrap().prepare(&paths).unwrap();
        let launch = plan.launch();
        let output = Command::new(&launch.program[0])
            .args(&launch.program[1..])
            .arg("-c")
            .arg("codex")
            .current_dir(&seed_cwd)
            .env_clear()
            .envs(&launch.environment)
            .env("PATH", &bin)
            .env("TERM", "xterm-256color")
            .env("WSNAV_RC_COUNT", &count)
            .env("WSNAV_PROBE_OUT", temporary.path().join("probe"))
            .output()
            .unwrap();

        assert!(
            output.status.success(),
            "{shell_name} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(fs::read_to_string(count).unwrap(), "x");
        assert_eq!(
            fs::read_to_string(temporary.path().join("probe")).unwrap(),
            expected_probe
        );
    }

    struct AccountMaterializationTmux {
        calls: RefCell<Vec<TmuxInvocation>>,
        wrapper: PathBuf,
        seed_cwd: PathBuf,
    }

    impl TmuxClient for AccountMaterializationTmux {
        fn invoke(&self, invocation: &TmuxInvocation) -> Result<TmuxResponse, RuntimeError> {
            let call = self.calls.borrow().len();
            if call == 0 {
                assert!(self.wrapper.is_file());
            }
            self.calls.borrow_mut().push(invocation.clone());
            let stdout = match call {
                0 | 1 => String::new(),
                2 => "%17\n".to_owned(),
                3 => "4242\n".to_owned(),
                4 => format!("{}\n", self.seed_cwd.display()),
                _ => {
                    return Err(RuntimeError::TmuxRejected(
                        "unexpected tmux call".to_owned(),
                    ));
                }
            };
            Ok(TmuxResponse {
                success: true,
                stdout,
                stderr: String::new(),
            })
        }
    }

    struct AccountMaterializationProcess;

    impl ProcessProbe for AccountMaterializationProcess {
        fn process_birth(&self, pid: u32) -> Option<String> {
            (pid == 4242).then(|| "birth-4242".to_owned())
        }
    }

    struct AccountMaterializationGroup;

    impl ProcessGroupProbe for AccountMaterializationGroup {
        fn process_group_checked(
            &self,
            pid: u32,
        ) -> Result<Option<ProcessGroupInfo>, ProcessProbeError> {
            Ok((pid == 4242).then_some(ProcessGroupInfo {
                process_group_id: 4242,
                session_id: 31337,
            }))
        }

        fn process_group_members_checked(
            &self,
            _group: &ProcessGroupInfo,
        ) -> Result<Vec<u32>, ProcessProbeError> {
            Ok(vec![4242])
        }

        fn process_group_members_by_id_checked(
            &self,
            _process_group_id: u32,
        ) -> Result<Vec<u32>, ProcessProbeError> {
            Ok(vec![4242])
        }
    }

    #[test]
    fn bash_launch_uses_only_the_exact_private_wrapper_and_original_home() {
        let temporary = tempfile::tempdir().unwrap();
        let seed_cwd = temporary.path().join("seed");
        let original_home = temporary.path().join("home");
        make_directory(&seed_cwd);
        make_directory(&original_home);
        let shell = temporary.path().join("bash");
        let wsnav = temporary.path().join("wsnav");
        executable(&shell);
        executable(&wsnav);
        let paths = RuntimePaths::for_runtime(temporary.path(), RuntimeId::new());

        let plan = AccountShellLaunch::new(
            &account_context(temporary.path()),
            &paths,
            &seed_cwd,
            &shell,
            &original_home,
            None,
            &wsnav,
        )
        .unwrap();
        let launch = plan.launch();

        assert_eq!(launch.cwd, fs::canonicalize(&seed_cwd).unwrap());
        assert_eq!(
            launch.program,
            vec![
                fs::canonicalize(&shell).unwrap().into_os_string(),
                OsString::from("--noprofile"),
                OsString::from("--rcfile"),
                paths.directory.join(BASH_WRAPPER_FILE).into_os_string(),
                OsString::from("-i"),
            ]
        );
        assert_eq!(
            environment_value(launch, ORIGINAL_HOME_ENV),
            Some(fs::canonicalize(&original_home).unwrap().into_os_string())
        );
        assert_eq!(
            environment_value(launch, HOME_ENV),
            Some(fs::canonicalize(&original_home).unwrap().into_os_string())
        );
        assert_eq!(
            environment_value(launch, STATE_ROOT_ENV),
            Some(fs::canonicalize(temporary.path()).unwrap().into_os_string())
        );
        assert_eq!(
            environment_value(launch, PRESENTATION_DIRECTORY_ENV),
            Some(
                fs::canonicalize(temporary.path().join("presentation"))
                    .unwrap()
                    .into_os_string()
            )
        );
        assert_eq!(
            environment_value(launch, EXECUTABLE_ENV),
            Some(fs::canonicalize(&wsnav).unwrap().into_os_string())
        );
        assert_eq!(environment_value(launch, "ZDOTDIR"), None);
        assert_eq!(environment_value(launch, ORIGINAL_ZDOTDIR_ENV), None);
    }

    #[test]
    fn zsh_launch_redirects_startup_then_restores_only_the_original_zdotdir() {
        let temporary = tempfile::tempdir().unwrap();
        let seed_cwd = temporary.path().join("seed");
        let original_home = temporary.path().join("home");
        let original_zdotdir = temporary.path().join("zdotdir");
        make_directory(&seed_cwd);
        make_directory(&original_home);
        make_directory(&original_zdotdir);
        let shell = temporary.path().join("zsh");
        let wsnav = temporary.path().join("wsnav");
        executable(&shell);
        executable(&wsnav);
        let paths = RuntimePaths::for_runtime(temporary.path(), RuntimeId::new());

        let plan = AccountShellLaunch::new(
            &account_context(temporary.path()),
            &paths,
            &seed_cwd,
            &shell,
            &original_home,
            Some(&original_zdotdir),
            &wsnav,
        )
        .unwrap();
        let launch = plan.launch();

        assert_eq!(
            launch.program,
            vec![
                fs::canonicalize(&shell).unwrap().into_os_string(),
                OsString::from("-i"),
            ]
        );
        assert_eq!(
            environment_value(launch, "ZDOTDIR"),
            Some(paths.directory.clone().into_os_string())
        );
        assert_eq!(
            environment_value(launch, ORIGINAL_ZDOTDIR_ENV),
            Some(
                fs::canonicalize(&original_zdotdir)
                    .unwrap()
                    .into_os_string()
            )
        );
    }

    #[test]
    fn zsh_launch_uses_original_home_when_zdotdir_is_unset() {
        let temporary = tempfile::tempdir().unwrap();
        let seed_cwd = temporary.path().join("seed");
        let original_home = temporary.path().join("home");
        make_directory(&seed_cwd);
        make_directory(&original_home);
        let shell = temporary.path().join("zsh");
        let wsnav = temporary.path().join("wsnav");
        executable(&shell);
        executable(&wsnav);
        let paths = RuntimePaths::for_runtime(temporary.path(), RuntimeId::new());

        let plan = AccountShellLaunch::new(
            &account_context(temporary.path()),
            &paths,
            &seed_cwd,
            &shell,
            &original_home,
            None,
            &wsnav,
        )
        .unwrap();

        assert_eq!(
            environment_value(plan.launch(), ORIGINAL_ZDOTDIR_ENV),
            Some(fs::canonicalize(&original_home).unwrap().into_os_string())
        );
    }

    #[test]
    fn bootstrap_writes_one_private_wrapper_only_for_its_bound_runtime() {
        let temporary = tempfile::tempdir().unwrap();
        let seed_cwd = temporary.path().join("seed");
        let original_home = temporary.path().join("home");
        make_directory(&seed_cwd);
        make_directory(&original_home);
        let shell = temporary.path().join("bash");
        let wsnav = temporary.path().join("wsnav");
        executable(&shell);
        executable(&wsnav);
        let paths = RuntimePaths::for_runtime(temporary.path(), RuntimeId::new());
        let other_paths = RuntimePaths::for_runtime(temporary.path(), RuntimeId::new());
        let plan = AccountShellLaunch::new(
            &account_context(temporary.path()),
            &paths,
            &seed_cwd,
            &shell,
            &original_home,
            None,
            &wsnav,
        )
        .unwrap();
        fs::create_dir_all(&paths.directory).unwrap();
        fs::create_dir_all(&other_paths.directory).unwrap();

        plan.bootstrap().prepare(&other_paths).unwrap_err();
        assert!(!other_paths.directory.join(BASH_WRAPPER_FILE).exists());
        plan.bootstrap().prepare(&paths).unwrap();

        let wrapper = paths.directory.join(BASH_WRAPPER_FILE);
        assert_eq!(fs::read_to_string(&wrapper).unwrap(), BASH_WRAPPER);
        assert!(matches!(
            plan.bootstrap().prepare(&paths),
            Err(RuntimeError::StartupUnavailable)
        ));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&wrapper).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn account_shell_materialization_writes_its_wrapper_before_the_private_server_starts() {
        let temporary = tempfile::tempdir().unwrap();
        let state_root = temporary.path().join("state");
        let presentation = state_root.join("presentation");
        let seed_cwd = temporary.path().join("seed");
        let original_home = temporary.path().join("home");
        fs::create_dir(&state_root).unwrap();
        fs::create_dir(&presentation).unwrap();
        make_directory(&seed_cwd);
        make_directory(&original_home);
        let shell = temporary.path().join("bash");
        let wsnav = temporary.path().join("wsnav");
        executable(&shell);
        executable(&wsnav);
        let runtime_id = RuntimeId::new();
        let slot = ProvisionalSlot::materializing(
            &state_root,
            Uuid::new_v4(),
            Revision::INITIAL,
            1,
            runtime_id,
            SlotGeneration::new(Uuid::new_v4()),
            &seed_cwd,
        )
        .unwrap();
        let paths = RuntimePaths::for_runtime(&state_root, runtime_id);
        let plan = AccountShellLaunch::new(
            &AccountShellContext::new(&state_root, &presentation).unwrap(),
            &paths,
            &seed_cwd,
            &shell,
            &original_home,
            None,
            &wsnav,
        )
        .unwrap();
        let tmux = AccountMaterializationTmux {
            calls: RefCell::new(Vec::new()),
            wrapper: paths.directory.join(BASH_WRAPPER_FILE),
            seed_cwd: fs::canonicalize(&seed_cwd).unwrap(),
        };
        let process_probe = AccountMaterializationProcess;
        let runtime = PrivateRuntime::new(&tmux, &process_probe, paths.clone());

        let materialized = materialize_private_shell_with_startup(
            &state_root,
            &presentation,
            &slot,
            &runtime,
            plan.launch(),
            plan.bootstrap(),
            &AccountMaterializationGroup,
        )
        .unwrap();

        assert_eq!(
            read_marker(&state_root, &presentation).unwrap(),
            materialized
        );
        assert_eq!(fs::read_to_string(&tmux.wrapper).unwrap(), BASH_WRAPPER);
        assert_eq!(tmux.calls.borrow().len(), 5);
    }

    #[test]
    fn unsupported_account_shell_is_refused_before_a_launch_plan_exists() {
        let temporary = tempfile::tempdir().unwrap();
        let seed_cwd = temporary.path().join("seed");
        let original_home = temporary.path().join("home");
        make_directory(&seed_cwd);
        make_directory(&original_home);
        let shell = temporary.path().join("fish");
        let wsnav = temporary.path().join("wsnav");
        executable(&shell);
        executable(&wsnav);
        let paths = RuntimePaths::for_runtime(temporary.path(), RuntimeId::new());

        assert_eq!(
            AccountShellLaunch::new(
                &account_context(temporary.path()),
                &paths,
                &seed_cwd,
                &shell,
                &original_home,
                None,
                &wsnav,
            )
            .unwrap_err(),
            AccountShellError::UnsupportedShell
        );
        assert!(!paths.directory.exists());
    }

    #[test]
    fn account_context_refuses_a_presentation_outside_its_state_root() {
        let temporary = tempfile::tempdir().unwrap();
        let state_root = temporary.path().join("state");
        let foreign_presentation = temporary.path().join("foreign-presentation");
        make_directory(&state_root);
        make_directory(&foreign_presentation);

        assert_eq!(
            AccountShellContext::new(&state_root, &foreign_presentation).unwrap_err(),
            AccountShellError::ContextUnavailable
        );
    }

    #[cfg(unix)]
    #[test]
    fn account_context_refuses_a_symlinked_presentation_discovery_path() {
        let temporary = tempfile::tempdir().unwrap();
        let state_root = temporary.path().join("state");
        let presentation = state_root.join("presentation");
        let alias = state_root.join("presentation-alias");
        make_directory(&state_root);
        make_directory(&presentation);
        std::os::unix::fs::symlink(&presentation, &alias).unwrap();

        assert_eq!(
            AccountShellContext::new(&state_root, &alias).unwrap_err(),
            AccountShellError::ContextUnavailable
        );
    }

    #[test]
    fn inherited_account_shell_context_is_canonicalized_and_requires_bounded_paths() {
        let temporary = tempfile::tempdir().unwrap();
        let state_root = temporary.path().join("state");
        let presentation = state_root.join("presentation");
        make_directory(&state_root);
        make_directory(&presentation);

        let context = AccountShellContext::from_environment_values(
            Some(state_root.clone().into_os_string()),
            Some(presentation.clone().into_os_string()),
        )
        .unwrap();
        assert_eq!(context.state_root(), fs::canonicalize(&state_root).unwrap());
        assert_eq!(
            context.presentation_directory(),
            fs::canonicalize(&presentation).unwrap()
        );
        assert_eq!(
            AccountShellContext::from_environment_values(None, Some(presentation.into_os_string()))
                .unwrap_err(),
            AccountShellError::ContextUnavailable
        );
        assert_eq!(
            AccountShellContext::from_environment_values(
                Some(OsString::from("unsafe\nstate")),
                Some(state_root.into_os_string()),
            )
            .unwrap_err(),
            AccountShellError::ContextUnavailable
        );
    }

    #[test]
    fn zsh_wrapper_body_is_the_only_startup_file_that_restores_zdotdir() {
        assert!(ZSH_WRAPPER.contains("export ZDOTDIR=\"${WSNAV_ORIGINAL_ZDOTDIR:?}\""));
        assert!(
            ZSH_WRAPPER.contains("_shell_gate --provider opencode --shell-leader-pid \"$$\" --")
        );
        assert!(
            BASH_WRAPPER
                .contains("_launch_helper --capability \"$wsnav_capability\" --provider codex --")
        );
        assert_eq!(ZSH_WRAPPER_FILE, ".zshrc");
    }

    #[test]
    fn bash_wrapper_sources_the_account_rc_once_then_preserves_unmanaged_provider_bypass() {
        run_controlled_shell("bash", 10, "unmanaged\n");
    }

    #[test]
    fn zsh_wrapper_sources_the_account_rc_once_then_preserves_unmanaged_provider_bypass() {
        run_controlled_shell("zsh", 10, "unmanaged\n");
    }

    #[test]
    fn bash_wrapper_execs_only_a_successfully_gated_hidden_launch_helper() {
        run_controlled_shell("bash", 0, "managed\n");
    }

    #[test]
    fn zsh_wrapper_execs_only_a_successfully_gated_hidden_launch_helper() {
        run_controlled_shell("zsh", 0, "managed\n");
    }
}
