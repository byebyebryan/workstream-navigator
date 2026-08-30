use super::{
    ATTACHMENT_STATUS_FILE, LinuxProcessProbe, MAX_ATTACHMENT_STATUS_BYTES_USIZE,
    MAX_PRESENTATION_ARTIFACT_ENTRIES, MAX_PRESENTATION_CONFIG_BYTES,
    MAX_PRESENTATION_OWNERSHIP_MARKER_BYTES, MAX_PROVISIONAL_MARKER_BYTES, MAX_TMUX_OUTPUT_BYTES,
    PRESENTATION_DIRECTORY, PRESENTATION_OWNERSHIP_MARKER_FILE, PROVISIONAL_MARKER_FILE, Path,
    Presentation, PresentationArtifactSet, PresentationError, PresentationMarker,
    PresentationOwnershipProof, PresentationPaths, PrivateRuntime, ProvisionalCleanupProof,
    ProvisionalLease, ProvisionalPhase, RuntimeProbe, StateRoot, SystemTmux,
    cancel_pre_handoff_under_lease, count_client_rows, fs, inspect_private_socket,
    inspect_regular_file, map_presentation_ownership_probe, open_current,
    optional_socket_identity_compatible, output_bounded, presentation_ownership_marker_path,
    presentation_session_name, private_tmux_command, read_marker, read_presentation_ownership,
    remove_exact_provisional_runtime_artifacts, remove_exact_regular_artifact,
    remove_exact_socket_artifact, retire_provider_exec_proven_marker, sanitize_diagnostic,
    validate_exact_provisional_runtime_artifacts,
};

pub(super) fn stopped_owned_presentation(presentation_live: bool) -> bool {
    !presentation_live
}

pub(super) fn should_reuse_presentation(session_live: bool, navigator_pane_dead: bool) -> bool {
    session_live && !navigator_pane_dead
}

pub(super) fn validate_presentation_artifact_entries(
    directory: &Path,
    artifacts: PresentationArtifactSet,
    marker: Option<&PresentationMarker>,
) -> Result<(), PresentationError> {
    let entries = fs::read_dir(directory).map_err(PresentationError::Io)?;
    for (count, entry) in entries
        .take(MAX_PRESENTATION_ARTIFACT_ENTRIES + 1)
        .enumerate()
    {
        if count >= MAX_PRESENTATION_ARTIFACT_ENTRIES {
            return Err(PresentationError::ControlRefused(
                "presentation directory contains too many artifacts",
            ));
        }
        let entry = entry.map_err(PresentationError::Io)?;
        let metadata = fs::symlink_metadata(entry.path()).map_err(PresentationError::Io)?;
        if metadata.file_type().is_symlink() {
            return Err(PresentationError::ControlRefused(
                "presentation directory contains a symlink",
            ));
        }
        let name = entry.file_name();
        if name != PRESENTATION_OWNERSHIP_MARKER_FILE
            && name != ATTACHMENT_STATUS_FILE
            && name != "tmux.conf"
            && name != "tmux.sock"
            && !(matches!(artifacts, PresentationArtifactSet::Current)
                && name == crate::provisional::PROVISIONAL_MARKER_FILE)
            && !(matches!(artifacts, PresentationArtifactSet::Current)
                && crate::review::is_review_artifact_name(&name))
        {
            return Err(PresentationError::ControlRefused(
                "presentation directory contains an unknown artifact",
            ));
        }
    }
    match artifacts {
        PresentationArtifactSet::Uninitialized => {
            if marker.is_some() {
                return Err(PresentationError::ControlRefused(
                    "presentation context is already initialized",
                ));
            }
        }
        PresentationArtifactSet::Current => {
            let current = marker.ok_or(PresentationError::ControlRefused(
                "presentation context is unavailable",
            ))?;
            crate::review::validate_artifacts(
                directory,
                current.presentation_id,
                current.presentation_revision,
            )
            .map_err(|_| {
                PresentationError::ControlRefused("observer review ownership is unavailable")
            })?;
        }
    }
    Ok(())
}

fn remove_owned_presentation(
    state_root: &Path,
    paths: &PresentationPaths,
    expected: &PresentationOwnershipProof,
    provisional: Option<&ProvisionalCleanupProof>,
) -> Result<(), PresentationError> {
    let actual = read_presentation_ownership(paths)?.ok_or(PresentationError::ControlRefused(
        "presentation ownership disappeared",
    ))?;
    if actual.marker != expected.marker || actual.marker_identity != expected.marker_identity {
        return Err(PresentationError::ControlRefused(
            "presentation ownership changed during close",
        ));
    }

    if let Some(provisional) = provisional {
        if read_marker(state_root, &paths.directory).map_err(|_| {
            PresentationError::ControlRefused("provisional shell cleanup is unavailable")
        })? != provisional.slot
        {
            return Err(PresentationError::ControlRefused(
                "provisional shell cleanup is unavailable",
            ));
        }
        validate_expected_artifacts(paths, expected)?;
        remove_exact_regular_artifact(
            &paths.directory.join(PROVISIONAL_MARKER_FILE),
            Some(&provisional.marker_identity),
            MAX_PROVISIONAL_MARKER_BYTES,
            &mut |_| Ok(()),
        )?;
    } else {
        match fs::symlink_metadata(paths.directory.join(PROVISIONAL_MARKER_FILE)) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Ok(_) => {
                return Err(PresentationError::ControlRefused(
                    "provisional shell appeared during close",
                ));
            }
            Err(error) => return Err(PresentationError::Io(error)),
        }
    }

    validate_expected_artifacts(paths, expected)?;
    let attachment = inspect_regular_file(
        &paths.attachment_status,
        false,
        MAX_ATTACHMENT_STATUS_BYTES_USIZE,
    )
    .map_err(map_presentation_ownership_probe)?;
    if let Some(identity) = attachment.as_ref() {
        remove_exact_regular_artifact(
            &paths.attachment_status,
            Some(identity),
            MAX_ATTACHMENT_STATUS_BYTES_USIZE,
            &mut |_| Ok(()),
        )?;
    }

    validate_expected_artifacts(paths, expected)?;
    remove_exact_regular_artifact(
        &paths.config,
        Some(&expected.marker.config_identity),
        MAX_PRESENTATION_CONFIG_BYTES,
        &mut |_| Ok(()),
    )?;

    validate_expected_artifacts(paths, expected)?;
    let socket = inspect_private_socket(&paths.socket).map_err(map_presentation_ownership_probe)?;
    if socket.is_some()
        && !optional_socket_identity_compatible(expected.socket_identity.as_ref(), socket.as_ref())
    {
        return Err(PresentationError::ControlRefused(
            "presentation socket identity changed during close",
        ));
    }
    if let Some(identity) = socket.as_ref() {
        remove_exact_socket_artifact(&paths.socket, Some(identity), &mut |_| Ok(()))?;
    }

    validate_expected_artifacts(paths, expected)?;
    remove_exact_regular_artifact(
        &presentation_ownership_marker_path(paths),
        Some(&expected.marker_identity),
        MAX_PRESENTATION_OWNERSHIP_MARKER_BYTES,
        &mut |_| Ok(()),
    )?;
    match fs::remove_dir(&paths.directory) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::DirectoryNotEmpty => {
            Err(PresentationError::ControlRefused(
                "presentation directory gained an entry during close",
            ))
        }
        Err(error) => Err(PresentationError::Io(error)),
    }
}

fn validate_expected_artifacts(
    paths: &PresentationPaths,
    expected: &PresentationOwnershipProof,
) -> Result<(), PresentationError> {
    validate_presentation_artifact_entries(
        &paths.directory,
        PresentationArtifactSet::Current,
        expected.marker.current.as_ref(),
    )
}

impl Presentation {
    /// Stops this owned presentation. A materialized, pre-handoff shell
    /// is cleaned only after the shared lease, current marker, presentation
    /// binding, and exact live shell all agree. Any handoff or runtime-owned
    /// phase is deliberately left for onboarding recovery rather than being
    /// mistaken for presentation cleanup authority.
    #[doc(hidden)]
    pub fn close(&self) -> Result<(), PresentationError> {
        let Some(ownership) = read_presentation_ownership(&self.paths)? else {
            match fs::symlink_metadata(&self.paths.directory) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                Ok(_) => {
                    return Err(PresentationError::ControlRefused(
                        "presentation ownership marker is missing or invalid",
                    ));
                }
                Err(error) => return Err(PresentationError::Io(error)),
            }
        };
        let current = read_presentation_ownership(&self.paths)?.ok_or(
            PresentationError::ControlRefused("presentation ownership disappeared before close"),
        )?;
        if current.marker != ownership.marker
            || current.marker_identity != ownership.marker_identity
            || (current.socket_identity.is_some()
                && (ownership.socket_identity.is_none()
                    || !optional_socket_identity_compatible(
                        ownership.socket_identity.as_ref(),
                        current.socket_identity.as_ref(),
                    )))
        {
            return Err(PresentationError::ControlRefused(
                "presentation ownership changed before close",
            ));
        }
        let context = self.context()?;

        let provisional = self.provisional_cleanup_proof()?;
        let provisional_lease = provisional
            .as_ref()
            .map(|provisional| self.cleanup_materialized_shell(provisional))
            .transpose()?;

        let result = self.invoke(None, vec!["kill-server".into()]);
        if let Err(PresentationError::TmuxRejected(message)) = &result
            && !message.contains("no server running")
            && !message.contains("No such file")
        {
            return Err(PresentationError::TmuxRejected(message.clone()));
        }
        if let Some(provisional_lease) = provisional_lease.as_ref() {
            provisional_lease
                .revalidate_for_mutation(&self.state_root)
                .map_err(|_| {
                    PresentationError::ControlRefused("provisional shell cleanup is unavailable")
                })?;
        }
        crate::review::recover_after_presentation_stop(
            &self.paths.directory,
            context.presentation_id(),
            context.presentation_revision(),
        )
        .map_err(|_| PresentationError::ControlRefused("observer review cleanup is unavailable"))?;
        remove_owned_presentation(
            &self.state_root,
            &self.paths,
            &ownership,
            provisional.as_ref(),
        )
    }

    fn provisional_cleanup_proof(
        &self,
    ) -> Result<Option<ProvisionalCleanupProof>, PresentationError> {
        let marker_path = self.paths.directory.join(PROVISIONAL_MARKER_FILE);
        let marker_identity =
            inspect_regular_file(&marker_path, false, MAX_PROVISIONAL_MARKER_BYTES)
                .map_err(map_presentation_ownership_probe)?;
        let Some(marker_identity) = marker_identity else {
            return Ok(None);
        };
        let slot = read_marker(&self.state_root, &self.paths.directory).map_err(|_| {
            PresentationError::ControlRefused("provisional shell cleanup is unavailable")
        })?;
        if slot.phase() == ProvisionalPhase::ProviderExecProven {
            let root = StateRoot::select(&self.state_root);
            let mut state = open_current(&root).map_err(|_| {
                PresentationError::ControlRefused("completed onboarding cleanup is unavailable")
            })?;
            let provisional_lease = state.acquire_provisional_lease().map_err(|_| {
                PresentationError::ControlRefused("completed onboarding cleanup is unavailable")
            })?;
            retire_provider_exec_proven_marker(
                &state,
                &provisional_lease,
                &self.paths.directory,
                &slot,
            )
            .map_err(|_| {
                PresentationError::ControlRefused("completed onboarding cleanup is unavailable")
            })?;
            return Ok(None);
        }
        if !matches!(
            slot.phase(),
            ProvisionalPhase::Materializing
                | ProvisionalPhase::Materialized
                | ProvisionalPhase::HandoffIssued
        ) {
            return Err(PresentationError::ControlRefused(
                "provisional shell cleanup requires onboarding recovery",
            ));
        }
        let context = self.context().map_err(|_| {
            PresentationError::ControlRefused("provisional shell cleanup is unavailable")
        })?;
        if slot.presentation_id() != context.presentation_id()
            || slot.presentation_revision() != context.presentation_revision()
            || slot.seed_cwd() != context.seed_cwd()
        {
            return Err(PresentationError::ControlRefused(
                "provisional shell cleanup is unavailable",
            ));
        }
        Ok(Some(ProvisionalCleanupProof {
            slot,
            marker_identity,
        }))
    }

    #[allow(clippy::too_many_lines)]
    fn cleanup_materialized_shell(
        &self,
        provisional: &ProvisionalCleanupProof,
    ) -> Result<ProvisionalLease, PresentationError> {
        let root = StateRoot::select(&self.state_root);
        let mut state = open_current(&root).map_err(|_| {
            PresentationError::ControlRefused(
                "provisional shell cleanup requires onboarding recovery",
            )
        })?;
        let provisional_lease = state.acquire_provisional_lease().map_err(|_| {
            PresentationError::ControlRefused("provisional shell cleanup is unavailable")
        })?;
        provisional_lease
            .revalidate_for_mutation(state.root())
            .map_err(|_| {
                PresentationError::ControlRefused("provisional shell cleanup is unavailable")
            })?;
        if read_marker(state.root(), &self.paths.directory).map_err(|_| {
            PresentationError::ControlRefused("provisional shell cleanup is unavailable")
        })? != provisional.slot
        {
            return Err(PresentationError::ControlRefused(
                "provisional shell cleanup is unavailable",
            ));
        }
        let context = self.context().map_err(|_| {
            PresentationError::ControlRefused("provisional shell cleanup is unavailable")
        })?;
        if provisional.slot.presentation_id() != context.presentation_id()
            || provisional.slot.presentation_revision() != context.presentation_revision()
            || provisional.slot.seed_cwd() != context.seed_cwd()
            || provisional.slot.lease_generation() != provisional_lease.lease_generation()
        {
            return Err(PresentationError::ControlRefused(
                "provisional shell cleanup is unavailable",
            ));
        }
        let tmux = SystemTmux::default();
        let process_probe = LinuxProcessProbe;
        let runtime = PrivateRuntime::new(
            &tmux,
            &process_probe,
            provisional.slot.runtime_paths().clone(),
        );
        let probe = runtime.probe().map_err(|_| {
            PresentationError::ControlRefused("provisional shell cleanup is unavailable")
        })?;
        if matches!(
            provisional.slot.phase(),
            ProvisionalPhase::Materialized | ProvisionalPhase::HandoffIssued
        ) && matches!(probe, RuntimeProbe::Live { .. })
        {
            provisional
                .slot
                .revalidate_live_shell(&runtime, &process_probe)
                .map_err(|_| {
                    PresentationError::ControlRefused("provisional shell cleanup is unavailable")
                })?;
        } else if matches!(probe, RuntimeProbe::Unknown { .. })
            || (provisional.slot.phase() == ProvisionalPhase::Materializing
                && matches!(probe, RuntimeProbe::Live { .. }))
        {
            return Err(PresentationError::ControlRefused(
                "provisional shell cleanup is unavailable",
            ));
        }
        cancel_pre_handoff_under_lease(
            &mut state,
            &provisional_lease,
            &self.paths.directory,
            &provisional.slot,
        )
        .map_err(|_| {
            PresentationError::ControlRefused("provisional shell cleanup is unavailable")
        })?;
        provisional_lease
            .revalidate_for_mutation(state.root())
            .map_err(|_| {
                PresentationError::ControlRefused("provisional shell cleanup is unavailable")
            })?;
        if matches!(probe, RuntimeProbe::Live { .. }) {
            validate_exact_provisional_runtime_artifacts(state.root(), &provisional.slot).map_err(
                |_| PresentationError::ControlRefused("provisional shell cleanup is unavailable"),
            )?;
            runtime.stop_server().map_err(|_| {
                PresentationError::ControlRefused("provisional shell cleanup is unavailable")
            })?;
        }
        remove_exact_provisional_runtime_artifacts(state.root(), &provisional.slot).map_err(
            |_| PresentationError::ControlRefused("provisional shell cleanup is unavailable"),
        )?;
        provisional_lease
            .revalidate_for_mutation(state.root())
            .map_err(|_| {
                PresentationError::ControlRefused("provisional shell cleanup is unavailable")
            })?;
        Ok(provisional_lease)
    }

    fn attached_client_count(&self) -> Result<usize, PresentationError> {
        let clients = self.invoke_capture(
            None,
            vec![
                "list-clients".into(),
                "-F".into(),
                "#{client_name}|#{session_name}|#{window_name}".into(),
            ],
        )?;
        count_client_rows(&clients, &self.paths.session_name)
            .map_err(map_presentation_ownership_probe)
    }

    pub(crate) fn discover_live(state_root: &Path) -> Result<Vec<Self>, PresentationError> {
        let presentation_root = state_root.join(PRESENTATION_DIRECTORY);
        if !presentation_root.exists() {
            return Ok(Vec::new());
        }
        let entries = fs::read_dir(&presentation_root).map_err(PresentationError::Io)?;
        let mut live = Vec::new();
        for entry in entries {
            let entry = entry.map_err(PresentationError::Io)?;
            if !entry.file_type().map_err(PresentationError::Io)?.is_dir() {
                return Err(PresentationError::InvalidControlPath(entry.path()));
            }
            let directory = entry.path();
            let session_name = presentation_session_name(&directory)
                .ok_or_else(|| PresentationError::InvalidControlPath(directory.clone()))?;
            let presentation =
                Self::from_control(state_root, directory.join("tmux.sock"), session_name)?;
            let session_live = presentation.is_live()?;
            let navigator_pane_dead = session_live && presentation.navigator_pane_is_dead()?;
            if should_reuse_presentation(session_live, navigator_pane_dead) {
                live.push(presentation);
                continue;
            }
            if session_live && presentation.attached_client_count()? > 0 {
                return Err(PresentationError::ControlRefused(
                    "presentation is attached while navigator recovery is pending",
                ));
            }
            presentation.close()?;
        }
        Ok(live)
    }

    pub(crate) fn is_live(&self) -> Result<bool, PresentationError> {
        let mut command = private_tmux_command();
        command.arg("-S").arg(&self.paths.socket).args([
            "has-session",
            "-t",
            &self.paths.session_name,
        ]);
        let output = output_bounded(&mut command, MAX_TMUX_OUTPUT_BYTES, MAX_TMUX_OUTPUT_BYTES)
            .map_err(PresentationError::from_bounded_tmux)?;
        if output.status.success() {
            return Ok(true);
        }
        let diagnostic = String::from_utf8_lossy(&output.stderr);
        if !self.paths.socket.exists()
            || diagnostic.contains("no server running")
            || diagnostic.contains("No such file")
        {
            return Ok(false);
        }
        Err(PresentationError::TmuxRejected(sanitize_diagnostic(
            &diagnostic,
        )))
    }
}
