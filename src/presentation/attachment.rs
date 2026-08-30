use super::{
    AttachmentPhase, AttachmentStatus, CurrentState, LinuxProcessProbe,
    MAX_ATTACHMENT_STATUS_BYTES, MAX_ATTACHMENT_STATUS_BYTES_USIZE, NAVIGATOR_STOP_ATTEMPTS,
    NAVIGATOR_STOP_RETRY, NAVIGATOR_WINDOW, OpenOptions, OsString, Path, Presentation,
    PresentationError, PresentationPaneRole, PrivateRuntime, ProvisionalLease, ProvisionalSlot,
    Read, Revision, RuntimeId, RuntimePaths, RuntimeProbe, StateRoot, SystemTmux, WorkstreamId,
    Write, fs, inspect_regular_file, map_presentation_ownership_probe, open_current,
    optional_socket_identity_compatible, private_tmux_command, read_presentation_ownership,
    remove_exact_regular_artifact, set_mode, stopped_owned_presentation, thread,
};

pub(super) fn prepare_attach_window_with_size<F>(
    session_name: &str,
    columns: u16,
    rows: u16,
    mut invoke: F,
) -> Result<(), PresentationError>
where
    F: FnMut(Vec<OsString>) -> Result<(), PresentationError>,
{
    if columns == 0 || rows == 0 {
        return Err(PresentationError::InvalidTerminalGeometry);
    }
    let target = format!("{session_name}:{NAVIGATOR_WINDOW}");
    invoke(vec![
        "resize-window".into(),
        "-t".into(),
        target.clone().into(),
        "-x".into(),
        columns.to_string().into(),
        "-y".into(),
        rows.to_string().into(),
    ])?;
    invoke(vec![
        "set-window-option".into(),
        "-t".into(),
        target.into(),
        "window-size".into(),
        "latest".into(),
    ])
}

impl Presentation {
    pub(super) fn provider_attach_command(
        &self,
        workstream_id: WorkstreamId,
        expected_workstream_revision: Revision,
        runtime_id: RuntimeId,
        expected_runtime_revision: Revision,
        attempt_id: uuid::Uuid,
    ) -> Vec<OsString> {
        vec![
            self.executable.clone().into_os_string(),
            "--state-root".into(),
            self.state_root.clone().into_os_string(),
            "_provider_attach".into(),
            workstream_id.to_string().into(),
            "--expected-workstream-revision".into(),
            expected_workstream_revision.value().to_string().into(),
            "--expected-runtime-id".into(),
            runtime_id.to_string().into(),
            "--expected-runtime-revision".into(),
            expected_runtime_revision.value().to_string().into(),
            "--presentation-socket".into(),
            self.paths.socket.clone().into_os_string(),
            "--presentation-session".into(),
            self.paths.session_name.clone().into(),
            "--attempt-id".into(),
            attempt_id.to_string().into(),
        ]
    }

    pub(super) fn prepare_attachment(
        &self,
        workstream_id: WorkstreamId,
    ) -> Result<AttachmentStatus, PresentationError> {
        let status = AttachmentStatus {
            attempt_id: uuid::Uuid::new_v4(),
            workstream_id,
            phase: AttachmentPhase::Pending,
        };
        self.write_attachment_status(&status)?;
        Ok(status)
    }

    pub(crate) fn finish_attachment_start(
        &self,
        mut status: AttachmentStatus,
        result: Result<(), PresentationError>,
    ) -> Result<AttachmentStatus, PresentationError> {
        if let Err(error) = result {
            status.phase = AttachmentPhase::Failed;
            let _ = self.write_attachment_status(&status);
            return Err(error);
        }
        Ok(status)
    }
}

impl Presentation {
    pub(super) fn read_attachment_status(
        &self,
    ) -> Result<Option<AttachmentStatus>, PresentationError> {
        let metadata = match fs::symlink_metadata(&self.paths.attachment_status) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(PresentationError::Io(error)),
        };
        if !metadata.file_type().is_file() || metadata.len() > MAX_ATTACHMENT_STATUS_BYTES {
            return Err(PresentationError::InvalidAttachmentStatus);
        }
        let file = fs::File::open(&self.paths.attachment_status).map_err(PresentationError::Io)?;
        let mut bytes = Vec::new();
        file.take(MAX_ATTACHMENT_STATUS_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(PresentationError::Io)?;
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_ATTACHMENT_STATUS_BYTES {
            return Err(PresentationError::InvalidAttachmentStatus);
        }
        let status: AttachmentStatus = serde_json::from_slice(&bytes)
            .map_err(|_| PresentationError::InvalidAttachmentStatus)?;
        Ok(Some(status))
    }

    pub(crate) fn write_attachment_status(
        &self,
        status: &AttachmentStatus,
    ) -> Result<(), PresentationError> {
        let bytes =
            serde_json::to_vec(status).map_err(|_| PresentationError::InvalidAttachmentStatus)?;
        if bytes.len() > usize::try_from(MAX_ATTACHMENT_STATUS_BYTES).unwrap_or(usize::MAX) {
            return Err(PresentationError::InvalidAttachmentStatus);
        }
        let temporary = self
            .paths
            .directory
            .join(format!(".attachment-{}.tmp", uuid::Uuid::new_v4().simple()));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary).map_err(PresentationError::Io)?;
        file.write_all(&bytes).map_err(PresentationError::Io)?;
        file.sync_all().map_err(PresentationError::Io)?;
        set_mode(&temporary, 0o600)?;
        fs::rename(&temporary, &self.paths.attachment_status).map_err(PresentationError::Io)
    }
}

impl Presentation {
    /// Returns the current exact provider attachment attempt. Before its helper
    /// reports `Running`, a dead pane is atomically converted to `Failed` for
    /// an exact same-row retry. Once running, the helper itself reports its
    /// terminal phase, so this method deliberately avoids repeated control
    /// queries against the presentation tmux server.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed private status or ambiguous tmux pane
    /// evidence.
    pub fn attachment_status(&self) -> Result<Option<AttachmentStatus>, PresentationError> {
        let Some(mut status) = self.read_attachment_status()? else {
            return Ok(None);
        };
        if status.phase == AttachmentPhase::Pending && self.provider_pane_is_dead()? {
            status.phase = AttachmentPhase::Failed;
            self.write_attachment_status(&status)?;
        }
        Ok(Some(status))
    }

    /// Stops the owned presentation after the Navigator pane has exited.
    /// A materialized provisional shell is an allowed presentation
    /// artifact, but remains a hard refusal for incomplete onboarding.
    pub(crate) fn stop_session(&self) -> Result<(), PresentationError> {
        self.context()?;
        let ownership = read_presentation_ownership(&self.paths)?.ok_or(
            PresentationError::ControlRefused("presentation ownership marker is missing"),
        )?;
        let current =
            read_presentation_ownership(&self.paths)?.ok_or(PresentationError::ControlRefused(
                "presentation ownership disappeared before session stop",
            ))?;
        if current.marker != ownership.marker
            || current.marker_identity != ownership.marker_identity
            || current.socket_identity.is_none()
            || !optional_socket_identity_compatible(
                ownership.socket_identity.as_ref(),
                current.socket_identity.as_ref(),
            )
        {
            return Err(PresentationError::ControlRefused(
                "presentation ownership changed before session stop",
            ));
        }
        self.invoke(
            None,
            vec![
                "detach-client".into(),
                "-s".into(),
                self.paths.session_name.clone().into(),
            ],
        )
    }

    /// Advances only the currently recorded exact attachment attempt.
    ///
    /// # Errors
    ///
    /// Returns an error for a stale attempt, invalid transition, or private
    /// status I/O failure.
    pub fn report_attachment_phase(
        &self,
        attempt_id: uuid::Uuid,
        phase: AttachmentPhase,
    ) -> Result<(), PresentationError> {
        let Some(mut status) = self.read_attachment_status()? else {
            return Err(PresentationError::StaleAttachmentAttempt);
        };
        if status.attempt_id != attempt_id
            || !matches!(
                (status.phase, phase),
                (
                    AttachmentPhase::Pending,
                    AttachmentPhase::Running | AttachmentPhase::Failed
                ) | (
                    AttachmentPhase::Running,
                    AttachmentPhase::Completed | AttachmentPhase::Failed
                )
            )
        {
            return Err(PresentationError::StaleAttachmentAttempt);
        }
        status.phase = phase;
        self.write_attachment_status(&status)
    }
}

impl Presentation {
    /// Directly attaches the caller's terminal to an owned presentation.
    pub(crate) fn attach(&self) -> Result<(), PresentationError> {
        self.prepare_attach()?;
        let status = private_tmux_command()
            .arg("-S")
            .arg(&self.paths.socket)
            .args(["attach-session", "-t", &self.paths.session_name])
            .status()
            .map_err(PresentationError::Io)?;
        if stopped_owned_presentation(self.is_live()?) {
            self.close()?;
            return Ok(());
        }
        if status.success() {
            for _ in 0..NAVIGATOR_STOP_ATTEMPTS {
                if self.navigator_pane_is_dead()? {
                    self.close()?;
                    return Ok(());
                }
                thread::sleep(NAVIGATOR_STOP_RETRY);
            }
            return Ok(());
        }
        if self.navigator_pane_is_dead()? {
            self.close()?;
            return Ok(());
        }
        Err(PresentationError::TmuxRejected(
            "presentation attach failed".to_owned(),
        ))
    }

    pub(crate) fn prepare_attach(&self) -> Result<(), PresentationError> {
        let (columns, rows) = crossterm::terminal::size()
            .map_err(|_| PresentationError::TerminalGeometryUnavailable)?;
        self.prepare_attach_with_size(columns, rows)
    }

    pub(super) fn prepare_attach_with_size(
        &self,
        columns: u16,
        rows: u16,
    ) -> Result<(), PresentationError> {
        prepare_attach_window_with_size(&self.paths.session_name, columns, rows, |arguments| {
            self.invoke(None, arguments)
        })?;
        self.install_control_bindings()
    }

    /// Replaces only the outer provider attachment helper. The managed Codex
    /// runtime remains in its own private tmux server.
    /// Replaces the outer provider pane with a -only attachment helper for
    /// one already-proven Runtime. The presentation marker must still be
    /// current, and the helper receives the exact snapshot revisions instead
    /// of reopening the retired schema-15 application facade.
    pub(crate) fn attach_workstream(
        &self,
        workstream_id: WorkstreamId,
        expected_workstream_revision: Revision,
        runtime_id: RuntimeId,
        expected_runtime_revision: Revision,
    ) -> Result<AttachmentStatus, PresentationError> {
        self.context()?;
        self.with_attachment_claim(|| {
            let status = self.prepare_attachment(workstream_id)?;
            let result = (|| {
                let provider = self.provider_target_for_attachment()?;
                self.set_pane_role(
                    &provider,
                    PresentationPaneRole::Provider,
                    Some(status.workstream_id),
                )?;
                self.invoke(
                    None,
                    self.provider_respawn_for_command(
                        &provider,
                        self.provider_attach_command(
                            workstream_id,
                            expected_workstream_revision,
                            runtime_id,
                            expected_runtime_revision,
                            status.attempt_id,
                        ),
                    ),
                )
            })();
            self.finish_attachment_start(status, result)
        })
    }

    /// Replaces only the outer provider pane with the exact private tmux
    /// client for a materialized account shell. The candidate remains
    /// unregistered: this does not create a Workstream, Runtime, attachment
    /// record, or provider effect.
    ///
    /// The caller retains the schema-15 provisional lease through this
    /// transition. The marker, lease, and presentation context are
    /// revalidated immediately before the outer pane changes, so a stale or
    /// foreign candidate can never be attached merely because its paths look
    /// like a Runtime.
    ///
    /// # Errors
    ///
    /// Returns an error when the exact marker/lease/context does not
    /// authorize this shell, or when the owned provider pane cannot be
    /// replaced.
    pub(crate) fn attach_provisional_shell(
        &self,
        state: &CurrentState,
        provisional_lease: &ProvisionalLease,
        slot: &ProvisionalSlot,
    ) -> Result<(), PresentationError> {
        self.with_attachment_claim(|| {
            self.validate_provisional_attachment(state, provisional_lease, slot)?;
            let provider = self.provider_target_for_attachment()?;
            self.set_pane_role(&provider, PresentationPaneRole::Provider, None)?;
            self.invoke(
                None,
                self.provider_respawn_for_command(
                    &provider,
                    Self::provisional_attach_command(slot.runtime_paths()),
                ),
            )?;
            // The provider pane has changed, but no state did. Recheck
            // the held lease before returning so the controller never treats
            // a changed lock as successful shell authority.
            provisional_lease
                .revalidate_for_mutation(state.root())
                .map_err(|_| {
                    PresentationError::ControlRefused("provisional shell attachment is unavailable")
                })?;
            self.provider_target_for_attachment()?;
            Ok(())
        })
    }

    /// Replaces an unattached provider pane with a native Codex process
    /// for the contextual observer `/hooks` review. The process receives only
    /// the exact owned profile home and a disposable review directory; no
    /// `WSNav` command, management traffic, or provider payload is sent into
    /// the pane. A live or ambiguous managed attachment is a hard refusal
    /// because its native output must remain visible and untouched; only an
    /// exact deliberately parked attachment may surrender its outer helper
    /// pane.
    #[allow(
        clippy::too_many_lines,
        reason = "The exact parked-attachment handoff keeps topology, state, status, and native review fences together."
    )]
    pub(crate) fn start_observer_review(
        &self,
        executable: &Path,
        codex_home: &Path,
        review_directory: &Path,
        detached_workstream_id: Option<WorkstreamId>,
    ) -> Result<(), PresentationError> {
        if !executable.is_absolute() || !codex_home.is_absolute() || !review_directory.is_absolute()
        {
            return Err(PresentationError::ControlRefused(
                "observer review paths are not exact",
            ));
        }
        let executable_metadata = fs::symlink_metadata(executable)
            .map_err(|_| PresentationError::ControlRefused("observer executable is unavailable"))?;
        let review_metadata = fs::symlink_metadata(review_directory)
            .map_err(|_| PresentationError::ControlRefused("observer review is unavailable"))?;
        if executable_metadata.file_type().is_symlink()
            || !executable_metadata.is_file()
            || review_metadata.file_type().is_symlink()
            || !review_metadata.is_dir()
        {
            return Err(PresentationError::ControlRefused(
                "observer review evidence is unavailable",
            ));
        }
        self.context()?;
        self.with_attachment_claim(|| {
            let topology = self.attachment_topology()?;
            let provider = topology
                .provider()
                .ok_or(PresentationError::InvalidTopology)?;
            let attached_workstream = self.observer_attachment_context()?;
            if attached_workstream != detached_workstream_id {
                return Err(PresentationError::ControlRefused(
                    "observer review attachment context changed",
                ));
            }
            if let Some(workstream_id) = attached_workstream {
                // A parked/stopped Runtime is the only managed attachment
                // that may surrender the outer helper pane.  The private
                // Runtime remains the durable reopen path; a live or
                // ambiguous probe keeps its provider output untouched.
                self.prove_stopped_attachment(workstream_id)?;
            }
            let previous_status = self.read_attachment_status()?;
            let role_result =
                self.set_pane_role(&provider.id, PresentationPaneRole::Provider, None);
            if let Err(error) = role_result {
                let _ = self.set_pane_role(
                    &provider.id,
                    PresentationPaneRole::Provider,
                    attached_workstream,
                );
                return Err(error);
            }
            if let Err(error) = self.clear_observer_attachment_status(attached_workstream) {
                let _ = self.set_pane_role(
                    &provider.id,
                    PresentationPaneRole::Provider,
                    attached_workstream,
                );
                if let Some(status) = previous_status.as_ref()
                    && matches!(self.read_attachment_status(), Ok(None))
                {
                    let _ = self.write_attachment_status(status);
                }
                return Err(error);
            }
            if let Err(error) = self.set_pane_remain_on_exit(&provider.id, true) {
                let _ = self.set_pane_role(
                    &provider.id,
                    PresentationPaneRole::Provider,
                    attached_workstream,
                );
                if let Some(status) = previous_status.as_ref()
                    && matches!(self.read_attachment_status(), Ok(None))
                {
                    let _ = self.write_attachment_status(status);
                }
                return Err(error);
            }
            let command = vec![
                "env".into(),
                "-u".into(),
                "TMUX".into(),
                format!("{}={}", "CODEX_HOME", codex_home.display()).into(),
                executable.as_os_str().to_owned(),
                "--profile".into(),
                crate::provider::codex::profile::OBSERVER_PROFILE_NAME.into(),
                "-C".into(),
                review_directory.as_os_str().to_owned(),
            ];
            let result = self.invoke(
                None,
                self.provider_respawn_for_command(&provider.id, command),
            );
            if let Err(error) = result {
                // Restore the exact outer attachment metadata when tmux
                // refuses the review respawn.  A changed status file is
                // deliberately not overwritten.
                let _ = self.set_pane_role(
                    &provider.id,
                    PresentationPaneRole::Provider,
                    attached_workstream,
                );
                if let Some(status) = previous_status.as_ref()
                    && matches!(self.read_attachment_status(), Ok(None))
                {
                    let _ = self.write_attachment_status(status);
                }
                return Err(error);
            }
            Ok(())
        })
    }

    /// Returns the exact current outer attachment context that can be
    /// detached for a native observer review.  A managed context is accepted
    /// only with its matching terminal attachment attempt; a running or
    /// mismatched helper is an explicit refusal.
    pub(crate) fn observer_attachment_context(
        &self,
    ) -> Result<Option<WorkstreamId>, PresentationError> {
        let topology = self.attachment_topology()?;
        let provider = topology
            .provider()
            .ok_or(PresentationError::InvalidTopology)?;
        let status = self.attachment_status()?;
        match (provider.workstream_id, status) {
            (None, None) => Ok(None),
            (Some(workstream_id), Some(status))
                if status.workstream_id == workstream_id
                    && matches!(
                        status.phase,
                        AttachmentPhase::Completed | AttachmentPhase::Failed
                    ) =>
            {
                Ok(Some(workstream_id))
            }
            _ => Err(PresentationError::ControlRefused(
                "observer review attachment evidence is unavailable",
            )),
        }
    }

    /// Proves that the managed Runtime whose outer helper is being replaced
    /// is deliberately parked and absent from its private tmux server.  The
    /// outer pane alone is not mutation authority: a stopped-looking helper
    /// can still belong to a live or changed Runtime.
    fn prove_stopped_attachment(
        &self,
        workstream_id: WorkstreamId,
    ) -> Result<(), PresentationError> {
        let root = StateRoot::select(&self.state_root);
        let state = open_current(&root).map_err(|_| {
            PresentationError::ControlRefused(
                "observer review managed attachment state is unavailable",
            )
        })?;
        let registry = state.into_host_registry().map_err(|_| {
            PresentationError::ControlRefused(
                "observer review managed attachment state is unavailable",
            )
        })?;
        let runtime = registry
            .runtime_for_workstream(workstream_id)
            .map_err(|_| {
                PresentationError::ControlRefused(
                    "observer review managed attachment state is unavailable",
                )
            })?
            .ok_or(PresentationError::ControlRefused(
                "observer review managed attachment is unavailable",
            ))?;
        let deliberately_parked = registry
            .runtime_is_deliberately_parked(runtime.runtime_id, workstream_id)
            .map_err(|_| {
                PresentationError::ControlRefused(
                    "observer review managed attachment state is unavailable",
                )
            })?;
        if !deliberately_parked {
            return Err(PresentationError::ControlRefused(
                "observer review cannot replace a live or unparked attachment",
            ));
        }
        let paths =
            RuntimePaths::for_record(root.base(), runtime.runtime_id, &runtime.tmux_session)
                .map_err(|_| {
                    PresentationError::ControlRefused(
                        "observer review managed attachment paths are unavailable",
                    )
                })?;
        let tmux = SystemTmux::default();
        let process_probe = LinuxProcessProbe;
        let private_runtime = PrivateRuntime::new(&tmux, &process_probe, paths);
        match private_runtime.probe().map_err(|_| {
            PresentationError::ControlRefused(
                "observer review managed attachment probe is unavailable",
            )
        })? {
            RuntimeProbe::Missing => Ok(()),
            RuntimeProbe::Live { .. } | RuntimeProbe::Unknown { .. } => {
                Err(PresentationError::ControlRefused(
                    "observer review cannot replace a live or ambiguous attachment",
                ))
            }
        }
    }

    /// Removes only the exact host-local attachment attempt that accompanied
    /// a stopped managed context.  A mismatched, running, malformed, or
    /// foreign status file remains untouched and blocks review.
    fn clear_observer_attachment_status(
        &self,
        workstream_id: Option<WorkstreamId>,
    ) -> Result<(), PresentationError> {
        let Some(workstream_id) = workstream_id else {
            if self.read_attachment_status()?.is_some() {
                return Err(PresentationError::ControlRefused(
                    "observer review attachment status is unexpected",
                ));
            }
            return Ok(());
        };
        let status = self
            .read_attachment_status()?
            .ok_or(PresentationError::ControlRefused(
                "observer review attachment status is missing",
            ))?;
        if status.workstream_id != workstream_id
            || !matches!(
                status.phase,
                AttachmentPhase::Completed | AttachmentPhase::Failed
            )
        {
            return Err(PresentationError::ControlRefused(
                "observer review attachment status changed",
            ));
        }
        let identity = inspect_regular_file(
            &self.paths.attachment_status,
            true,
            MAX_ATTACHMENT_STATUS_BYTES_USIZE,
        )
        .map_err(map_presentation_ownership_probe)?
        .ok_or(PresentationError::ControlRefused(
            "observer review attachment status disappeared",
        ))?;
        remove_exact_regular_artifact(
            &self.paths.attachment_status,
            Some(&identity),
            MAX_ATTACHMENT_STATUS_BYTES_USIZE,
            &mut |_| Ok(()),
        )?;
        if self.read_attachment_status()?.is_some() {
            return Err(PresentationError::ControlRefused(
                "observer review attachment status changed",
            ));
        }
        Ok(())
    }

    /// Reports whether the exact provider pane used for a contextual observer
    /// review has exited. The pane is retained by tmux, so this is evidence
    /// only; native trust is still verified from the exact profile before any
    /// pending managed action can resume.
    pub(crate) fn observer_review_finished(&self) -> Result<bool, PresentationError> {
        let topology = self.read_topology_allow_dead()?;
        let provider = topology
            .provider()
            .ok_or(PresentationError::InvalidTopology)?;
        if provider.workstream_id.is_some() {
            return Err(PresentationError::ControlRefused(
                "observer review provider context changed",
            ));
        }
        Ok(provider.dead)
    }
}
