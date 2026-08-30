use super::{
    ATTACHMENT_STATUS_FILE, INITIAL_PRESENTATION_HEIGHT, INITIAL_PRESENTATION_WIDTH,
    MAX_ATTACHMENT_STATUS_BYTES_USIZE, MAX_PRESENTATION_CONFIG_BYTES,
    MAX_PRESENTATION_OWNERSHIP_MARKER_BYTES, NAVIGATOR_PANE, NAVIGATOR_WINDOW, OpenOptions,
    OsString, PREFERRED_PROVIDER_PANE_WIDTH, PRESENTATION_CONTEXT_VERSION, PRESENTATION_DIRECTORY,
    PRESENTATION_OWNERSHIP_MARKER_FILE, PROVIDER_PANE, Path, PathBuf, Presentation,
    PresentationContext, PresentationError, PresentationFileIdentity, PresentationMarker,
    PresentationOwnershipMarker, PresentationOwnershipProof, PresentationPaneRole,
    PresentationPaths, Read, Revision, Seek, SeekFrom, Write, config_content_matches,
    directory_identity_compatible, file_device, file_inode, fs, inspect_private_socket,
    inspect_regular_file, is_private_owner_directory, is_private_owner_file,
    map_presentation_ownership_probe, optional_socket_identity_compatible,
    presentation_file_identity, presentation_session_name, presentation_tmux_config,
    read_private_file, set_mode, sync_directory, validate_presentation_artifact_entries,
};

impl Presentation {
    pub(super) fn navigator_command(&self) -> Vec<OsString> {
        self.navigator_command_for("_navigator")
    }

    pub(super) fn new_session_arguments(&self) -> Vec<OsString> {
        let mut arguments = vec![
            "new-session".into(),
            "-d".into(),
            "-x".into(),
            INITIAL_PRESENTATION_WIDTH.to_string().into(),
            "-y".into(),
            INITIAL_PRESENTATION_HEIGHT.to_string().into(),
            "-s".into(),
            self.paths.session_name.clone().into(),
            "-n".into(),
            NAVIGATOR_WINDOW.into(),
        ];
        arguments.extend(self.navigator_command());
        arguments
    }

    fn navigator_command_for(&self, pane_command: &str) -> Vec<OsString> {
        vec![
            self.executable.clone().into_os_string(),
            "--state-root".into(),
            self.state_root.clone().into_os_string(),
            pane_command.into(),
            "--presentation-socket".into(),
            self.paths.socket.clone().into_os_string(),
            "--presentation-session".into(),
            self.paths.session_name.clone().into(),
        ]
    }

    fn provider_wait_command(&self) -> Vec<OsString> {
        vec![
            self.executable.clone().into_os_string(),
            "--state-root".into(),
            self.state_root.clone().into_os_string(),
            "_provider_wait".into(),
        ]
    }
}

fn canonical_seed_cwd(seed_cwd: &Path) -> Result<PathBuf, PresentationError> {
    let seed_cwd = fs::canonicalize(seed_cwd).map_err(|_| PresentationError::SeedUnavailable)?;
    if !seed_cwd.is_dir() {
        return Err(PresentationError::SeedUnavailable);
    }
    Ok(seed_cwd)
}

fn context_from_marker(
    marker: &PresentationMarker,
) -> Result<PresentationContext, PresentationError> {
    if marker.version != PRESENTATION_CONTEXT_VERSION
        || marker.presentation_id.is_nil()
        || marker.presentation_revision.value() < Revision::INITIAL.value()
    {
        return Err(PresentationError::ContextInvalid);
    }
    let seed_cwd = canonical_seed_cwd(&marker.seed_cwd)?;
    if seed_cwd != marker.seed_cwd {
        return Err(PresentationError::ContextInvalid);
    }
    Ok(PresentationContext {
        presentation_id: marker.presentation_id,
        presentation_revision: marker.presentation_revision,
        seed_cwd,
    })
}

pub(super) fn create_paths(paths: &PresentationPaths) -> Result<(), PresentationError> {
    let parent = paths
        .directory
        .parent()
        .ok_or_else(|| PresentationError::InvalidControlPath(paths.directory.clone()))?;
    fs::create_dir_all(parent).map_err(PresentationError::Io)?;
    set_mode(parent, 0o700)?;
    fs::create_dir(&paths.directory).map_err(PresentationError::Io)?;
    set_mode(&paths.directory, 0o700)?;
    let config = presentation_tmux_config();
    fs::write(&paths.config, &config).map_err(PresentationError::Io)?;
    set_mode(&paths.config, 0o600)?;

    let directory_metadata =
        fs::symlink_metadata(&paths.directory).map_err(PresentationError::Io)?;
    let config_metadata = fs::symlink_metadata(&paths.config).map_err(PresentationError::Io)?;
    let marker = PresentationOwnershipMarker {
        version: 1,
        directory: paths.directory.clone(),
        socket: paths.socket.clone(),
        session_name: paths.session_name.clone(),
        directory_identity: presentation_file_identity(&directory_metadata, None),
        config_identity: presentation_file_identity(&config_metadata, Some(config.as_bytes())),
        socket_identity: None,
        current: None,
    };
    write_presentation_ownership_marker(paths, &marker, None)
}

#[cfg(test)]
pub(crate) fn create_paths_for_test(paths: &PresentationPaths) -> Result<(), PresentationError> {
    create_paths(paths)
}

pub(super) fn presentation_ownership_marker_path(paths: &PresentationPaths) -> PathBuf {
    paths.directory.join(PRESENTATION_OWNERSHIP_MARKER_FILE)
}

pub(super) fn write_presentation_ownership_marker(
    paths: &PresentationPaths,
    marker: &PresentationOwnershipMarker,
    expected_identity: Option<&PresentationFileIdentity>,
) -> Result<(), PresentationError> {
    let bytes = serde_json::to_vec(marker).map_err(|_| {
        PresentationError::ControlRefused("presentation ownership marker could not be encoded")
    })?;
    if bytes.len() > MAX_PRESENTATION_OWNERSHIP_MARKER_BYTES {
        return Err(PresentationError::ControlRefused(
            "presentation ownership marker exceeded its bound",
        ));
    }
    let marker_path = presentation_ownership_marker_path(paths);
    let Some(expected_identity) = expected_identity else {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&marker_path).map_err(PresentationError::Io)?;
        file.write_all(&bytes).map_err(PresentationError::Io)?;
        file.sync_all().map_err(PresentationError::Io)?;
        set_mode(&marker_path, 0o600)?;
        return sync_directory(&paths.directory);
    };
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(nix::libc::O_CLOEXEC | nix::libc::O_NOFOLLOW);
    }
    let mut file = options.open(&marker_path).map_err(PresentationError::Io)?;
    let opened = file.metadata().map_err(PresentationError::Io)?;
    let mut before_bytes = Vec::new();
    (&mut file)
        .take((MAX_PRESENTATION_OWNERSHIP_MARKER_BYTES as u64).saturating_add(1))
        .read_to_end(&mut before_bytes)
        .map_err(PresentationError::Io)?;
    if before_bytes.len() > MAX_PRESENTATION_OWNERSHIP_MARKER_BYTES
        || presentation_file_identity(&opened, Some(&before_bytes)) != *expected_identity
        || !opened.is_file()
        || !is_private_owner_file(&opened)
    {
        return Err(PresentationError::ControlRefused(
            "presentation ownership changed before marker update",
        ));
    }
    file.set_len(0).map_err(PresentationError::Io)?;
    file.seek(SeekFrom::Start(0))
        .map_err(PresentationError::Io)?;
    file.write_all(&bytes).map_err(PresentationError::Io)?;
    file.sync_all().map_err(PresentationError::Io)?;
    let after = fs::symlink_metadata(&marker_path).map_err(PresentationError::Io)?;
    if file_device(&after) != expected_identity.device
        || file_inode(&after) != expected_identity.inode
        || !after.is_file()
        || !is_private_owner_file(&after)
    {
        return Err(PresentationError::ControlRefused(
            "presentation ownership changed during marker update",
        ));
    }
    sync_directory(&paths.directory)
}

fn read_uninitialized_presentation_ownership(
    paths: &PresentationPaths,
) -> Result<Option<PresentationOwnershipProof>, PresentationError> {
    read_presentation_ownership_with_artifacts(paths, PresentationArtifactSet::Uninitialized)
}

/// Reads an owned presentation after a provisional marker may exist.
pub(super) fn read_presentation_ownership(
    paths: &PresentationPaths,
) -> Result<Option<PresentationOwnershipProof>, PresentationError> {
    read_presentation_ownership_with_artifacts(paths, PresentationArtifactSet::Current)
}

#[derive(Clone, Copy)]
pub(super) enum PresentationArtifactSet {
    Uninitialized,
    Current,
}

fn read_presentation_ownership_with_artifacts(
    paths: &PresentationPaths,
    artifacts: PresentationArtifactSet,
) -> Result<Option<PresentationOwnershipProof>, PresentationError> {
    let directory_metadata = match fs::symlink_metadata(&paths.directory) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(PresentationError::Io(error)),
    };
    if directory_metadata.file_type().is_symlink()
        || !directory_metadata.is_dir()
        || !is_private_owner_directory(&directory_metadata)
    {
        return Err(PresentationError::ControlRefused(
            "presentation ownership directory is foreign or malformed",
        ));
    }
    let marker_path = presentation_ownership_marker_path(paths);
    let marker_metadata = match fs::symlink_metadata(&marker_path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(PresentationError::Io(error)),
    };
    if marker_metadata.file_type().is_symlink()
        || !marker_metadata.is_file()
        || !is_private_owner_file(&marker_metadata)
        || marker_metadata.len() > MAX_PRESENTATION_OWNERSHIP_MARKER_BYTES as u64
    {
        return Err(PresentationError::ControlRefused(
            "presentation ownership marker is foreign or malformed",
        ));
    }
    let bytes = read_private_file(&marker_path, MAX_PRESENTATION_OWNERSHIP_MARKER_BYTES)
        .map_err(map_presentation_ownership_probe)?
        .ok_or(PresentationError::ControlRefused(
            "presentation ownership marker disappeared",
        ))?;
    let marker_after = fs::symlink_metadata(&marker_path).map_err(PresentationError::Io)?;
    if marker_after.file_type().is_symlink()
        || !marker_after.is_file()
        || !is_private_owner_file(&marker_after)
        || marker_after.len() != marker_metadata.len()
        || file_device(&marker_after) != file_device(&marker_metadata)
        || file_inode(&marker_after) != file_inode(&marker_metadata)
    {
        return Err(PresentationError::ControlRefused(
            "presentation ownership marker changed during inspection",
        ));
    }
    let marker: PresentationOwnershipMarker = serde_json::from_slice(&bytes).map_err(|_| {
        PresentationError::ControlRefused("presentation ownership marker is malformed")
    })?;
    if marker.version != 1
        || marker.directory != paths.directory
        || marker.socket != paths.socket
        || marker.session_name != paths.session_name
        || marker.config_identity.mode != 0o600
        || marker.directory_identity.mode != 0o700
        || !directory_identity_compatible(
            &marker.directory_identity,
            &presentation_file_identity(&directory_metadata, None),
        )
    {
        return Err(PresentationError::ControlRefused(
            "presentation ownership marker does not prove this directory",
        ));
    }
    validate_presentation_artifact_entries(&paths.directory, artifacts, marker.current.as_ref())?;
    let config = inspect_regular_file(&paths.config, true, MAX_PRESENTATION_CONFIG_BYTES)
        .map_err(map_presentation_ownership_probe)?
        .ok_or(PresentationError::ControlRefused(
            "presentation configuration is missing",
        ))?;
    if config != marker.config_identity || !config_content_matches(&config) {
        return Err(PresentationError::ControlRefused(
            "presentation configuration is foreign or modified",
        ));
    }
    let socket = inspect_private_socket(&paths.socket).map_err(map_presentation_ownership_probe)?;
    if socket.is_some()
        && marker.socket_identity.is_some()
        && !optional_socket_identity_compatible(marker.socket_identity.as_ref(), socket.as_ref())
    {
        return Err(PresentationError::ControlRefused(
            "presentation socket identity changed",
        ));
    }
    if let Some(attachment) = inspect_regular_file(
        &paths.attachment_status,
        false,
        MAX_ATTACHMENT_STATUS_BYTES_USIZE,
    )
    .map_err(map_presentation_ownership_probe)?
    {
        let _ = attachment;
    }
    let marker_identity = presentation_file_identity(&marker_after, Some(&bytes));
    Ok(Some(PresentationOwnershipProof {
        marker,
        marker_identity,
        socket_identity: socket,
    }))
}

impl Presentation {
    fn capture_ownership_socket_identity(&self) -> Result<(), PresentationError> {
        let Some(mut ownership) = read_presentation_ownership(&self.paths)? else {
            return Err(PresentationError::ControlRefused(
                "presentation ownership marker disappeared",
            ));
        };
        let socket = inspect_private_socket(&self.paths.socket)
            .map_err(map_presentation_ownership_probe)?
            .ok_or(PresentationError::ControlRefused(
                "private presentation socket is missing",
            ))?;
        ownership.marker.socket_identity = Some(socket);
        write_presentation_ownership_marker(
            &self.paths,
            &ownership.marker,
            Some(&ownership.marker_identity),
        )
    }
}

impl Presentation {
    /// Creates an unstarted presentation owner for the current executable.
    ///
    /// # Errors
    ///
    /// Returns an error when the current executable cannot be resolved.
    pub fn fresh(state_root: &Path) -> Result<Self, PresentationError> {
        let executable = std::env::current_exe().map_err(PresentationError::Io)?;
        Ok(Self::fresh_with_executable(state_root, executable))
    }

    /// Creates an owner with an explicitly fixed executable. This is used by
    /// disposable integration fixtures so a test harness can exercise the
    /// real hidden helper instead of becoming the helper itself.
    #[doc(hidden)]
    #[must_use]
    pub fn fresh_with_executable(state_root: &Path, executable: PathBuf) -> Self {
        Self {
            paths: PresentationPaths::fresh(state_root),
            executable,
            state_root: state_root.to_path_buf(),
        }
    }

    /// presentation discovery and opener. Its discovery and cleanup path
    /// admits the presentation-private provisional marker.
    pub(crate) fn open_or_create(state_root: &Path) -> Result<(Self, bool), PresentationError> {
        let live = Self::discover_live(state_root)?;
        match live.as_slice() {
            [] => Ok((Self::fresh(state_root)?, true)),
            [presentation] => Ok((presentation.clone(), false)),
            _ => Err(PresentationError::AmbiguousPresentations),
        }
    }

    /// Reopens the exact owned presentation described by a hidden child
    /// command. This does not discover or use any ordinary tmux socket.
    ///
    /// # Errors
    ///
    /// Returns an error when the executable cannot be resolved or the supplied
    /// control values do not name an owned private presentation.
    pub fn from_control(
        state_root: &Path,
        socket: PathBuf,
        session_name: String,
    ) -> Result<Self, PresentationError> {
        Ok(Self {
            paths: PresentationPaths::from_control(state_root, socket, session_name)?,
            executable: std::env::current_exe().map_err(PresentationError::Io)?,
            state_root: state_root.to_path_buf(),
        })
    }

    #[must_use]
    pub fn paths(&self) -> &PresentationPaths {
        &self.paths
    }

    /// Captures the exact seed cwd for a fresh presentation. The context
    /// is embedded in the already-proven ownership marker, so later
    /// materialization can bind its provisional slot without deriving identity
    /// from a directory name or a provider process.
    ///
    /// This boundary neither creates a provisional server nor opens host state.
    pub(crate) fn initialize_context(
        &self,
        presentation_id: uuid::Uuid,
        seed_cwd: &Path,
    ) -> Result<PresentationContext, PresentationError> {
        let seed_cwd = canonical_seed_cwd(seed_cwd)?;
        if presentation_id.is_nil() {
            return Err(PresentationError::ContextInvalid);
        }
        let mut ownership = match read_uninitialized_presentation_ownership(&self.paths) {
            Ok(Some(ownership)) => ownership,
            Ok(None) => return Err(PresentationError::ContextUnavailable),
            Err(error) => {
                // The uninitialized reader deliberately rejects an existing
                // marker. Re-read through the current-owned reader so a
                // repeated initialization reports the typed state error while
                // preserving fail-closed handling for every other defect.
                if let Ok(Some(ownership)) = read_presentation_ownership(&self.paths)
                    && ownership.marker.current.is_some()
                {
                    return Err(PresentationError::ContextAlreadyInitialized);
                }
                return Err(error);
            }
        };
        if ownership.marker.current.is_some() {
            return Err(PresentationError::ContextAlreadyInitialized);
        }
        let marker = PresentationMarker {
            version: PRESENTATION_CONTEXT_VERSION,
            presentation_id,
            presentation_revision: Revision::INITIAL,
            seed_cwd,
        };
        let context = context_from_marker(&marker)?;
        ownership.marker.current = Some(marker);
        write_presentation_ownership_marker(
            &self.paths,
            &ownership.marker,
            Some(&ownership.marker_identity),
        )?;
        Ok(context)
    }

    /// Reopens the bounded context from the exact current presentation
    /// marker. It exposes no terminal data, provider input, or registry path.
    pub(crate) fn context(&self) -> Result<PresentationContext, PresentationError> {
        let ownership = read_presentation_ownership(&self.paths)?
            .ok_or(PresentationError::ContextUnavailable)?;
        let marker = ownership
            .marker
            .current
            .as_ref()
            .ok_or(PresentationError::ContextUnavailable)?;
        context_from_marker(marker)
    }

    /// Reopens the context only from an exact owned presentation directory
    /// beneath this state root. The inherited shell path is discovery input,
    /// not authority: this repeats the private ownership-marker proof before
    /// a shell gate may open schema-15 state.
    pub(crate) fn context_from_directory(
        state_root: &Path,
        presentation_directory: &Path,
    ) -> Result<PresentationContext, PresentationError> {
        let state_metadata =
            fs::symlink_metadata(state_root).map_err(|_| PresentationError::ContextUnavailable)?;
        if state_metadata.file_type().is_symlink() || !state_metadata.is_dir() {
            return Err(PresentationError::ContextUnavailable);
        }
        let state_root =
            fs::canonicalize(state_root).map_err(|_| PresentationError::ContextUnavailable)?;
        if !state_root.is_dir() {
            return Err(PresentationError::ContextUnavailable);
        }
        let original = fs::symlink_metadata(presentation_directory)
            .map_err(|_| PresentationError::ContextUnavailable)?;
        if original.file_type().is_symlink() || !original.is_dir() {
            return Err(PresentationError::ContextUnavailable);
        }
        let presentation_directory = fs::canonicalize(presentation_directory)
            .map_err(|_| PresentationError::ContextUnavailable)?;
        let presentation_root = state_root.join(PRESENTATION_DIRECTORY);
        if presentation_directory.parent() != Some(presentation_root.as_path()) {
            return Err(PresentationError::ContextUnavailable);
        }
        let session_name = presentation_session_name(&presentation_directory)
            .ok_or(PresentationError::ContextUnavailable)?;
        let paths = PresentationPaths {
            socket: presentation_directory.join("tmux.sock"),
            config: presentation_directory.join("tmux.conf"),
            attachment_status: presentation_directory.join(ATTACHMENT_STATUS_FILE),
            session_name,
            directory: presentation_directory,
        };
        let ownership =
            read_presentation_ownership(&paths)?.ok_or(PresentationError::ContextUnavailable)?;
        let marker = ownership
            .marker
            .current
            .as_ref()
            .ok_or(PresentationError::ContextUnavailable)?;
        context_from_marker(marker)
    }

    /// Starts a fresh private presentation with its seed context written
    /// before the navigator pane can run. This ordering prevents the pane from
    /// deriving a seed or identity after its process has already started.
    #[doc(hidden)]
    pub fn start(
        &self,
        presentation_id: uuid::Uuid,
        seed_cwd: &Path,
    ) -> Result<(), PresentationError> {
        self.start_with_context(presentation_id, seed_cwd)
            .map(|_| ())
    }

    pub(crate) fn start_with_context(
        &self,
        presentation_id: uuid::Uuid,
        seed_cwd: &Path,
    ) -> Result<PresentationContext, PresentationError> {
        create_paths(&self.paths)?;
        let context = self.complete_start_stage(
            "presentation context capture",
            self.initialize_context(presentation_id, seed_cwd),
        )?;
        let result = self.invoke(Some(&self.paths.config), self.new_session_arguments());
        self.complete_start_stage("server creation", result)?;
        let result = self.capture_ownership_socket_identity();
        self.complete_start_stage("socket ownership capture", result)?;
        let result = self
            .set_pane_role(NAVIGATOR_PANE, PresentationPaneRole::Navigator, None)
            .and_then(|()| self.set_pane_remain_on_exit(NAVIGATOR_PANE, true));
        self.complete_start_stage("navigator pane setup", result)?;
        let wait = self.provider_wait_command();
        let result = self.invoke(
            None,
            vec![
                "split-window".into(),
                "-h".into(),
                "-d".into(),
                "-t".into(),
                format!("{}:0.0", self.paths.session_name).into(),
                "-l".into(),
                PREFERRED_PROVIDER_PANE_WIDTH.to_string().into(),
                wait[0].clone(),
                wait[1].clone(),
                wait[2].clone(),
                wait[3].clone(),
            ],
        );
        self.complete_start_stage("provider pane creation", result)?;
        let result = self
            .set_pane_role(PROVIDER_PANE, PresentationPaneRole::Provider, None)
            .and_then(|()| self.set_pane_remain_on_exit(PROVIDER_PANE, true))
            .and_then(|()| self.install_control_bindings());
        self.complete_start_stage("provider pane setup", result)?;
        let result = self.retry_default_navigator_width();
        self.complete_start_stage("default navigator width", result)?;
        let result = self.install_navigator_width_hooks();
        self.complete_start_stage("navigator width hooks", result)?;
        Ok(context)
    }

    pub(super) fn complete_start_stage<T>(
        &self,
        stage: &'static str,
        result: Result<T, PresentationError>,
    ) -> Result<T, PresentationError> {
        result.map_err(|source| {
            // writes its marker before starting tmux. Once that marker is
            // present, cleanup stays on the owner and may reconcile an
            // interrupted provisional shell.
            let marker_exists = fs::symlink_metadata(
                self.paths
                    .directory
                    .join(PRESENTATION_OWNERSHIP_MARKER_FILE),
            )
            .is_ok();
            if marker_exists {
                let _ = self.close();
            }
            PresentationError::StartupFailed {
                stage,
                source: Box::new(source),
            }
        })
    }
}
