use super::topology::Direction;
use super::{
    COPY_MODE_SCROLL_BINDINGS, CurrentState, DEFAULT_NAVIGATOR_PANE_WIDTH, MAX_TMUX_OUTPUT_BYTES,
    NAVIGATOR_WIDTH_HOOKS, NAVIGATOR_WINDOW, OsString, PRESENTATION_CLAIM_OPTION, Path,
    Presentation, PresentationAction, PresentationError, PresentationPaneRole,
    PresentationTopology, ProvisionalLease, ProvisionalPhase, ProvisionalSlot, ROLE_OPTION,
    TMUX_FIELD_SEPARATOR, TOPOLOGY_FORMAT, WORKSTREAM_OPTION, WorkstreamId, output_bounded,
    parse_topology, parse_topology_with_dead, private_tmux_command, read_marker,
    sanitize_diagnostic, shell_quote,
};

impl Presentation {
    pub(crate) fn provider_pane_is_dead(&self) -> Result<bool, PresentationError> {
        let topology = self.read_topology_allow_dead()?;
        topology
            .provider()
            .map(|pane| pane.dead)
            .ok_or(PresentationError::InvalidTopology)
    }

    pub(crate) fn navigator_pane_is_dead(&self) -> Result<bool, PresentationError> {
        let topology = self.read_topology_allow_dead()?;
        topology
            .navigator()
            .map(|pane| pane.dead)
            .ok_or(PresentationError::InvalidTopology)
    }

    #[cfg(test)]
    pub(super) fn pane_dead_arguments(&self, pane: &str) -> Vec<OsString> {
        vec![
            "display-message".into(),
            "-p".into(),
            "-t".into(),
            self.pane_target(pane).into(),
            "#{pane_dead}".into(),
        ]
    }

    /// Keeps the narrow navigator at its deliberate default width, leaving
    /// all remaining terminal columns to the native provider pane.
    /// Reapplies the compact navigator layout after tmux adopts a controlling
    /// client's terminal size.
    ///
    /// # Errors
    ///
    /// Returns an error when the exact private tmux server rejects the resize.
    pub fn set_default_navigator_width(&self) -> Result<(), PresentationError> {
        let navigator = self.navigator_target()?;
        self.invoke(
            None,
            self.default_navigator_resize_arguments_for(&navigator),
        )
    }

    /// Reapplies the compact Navigator width across the short topology window
    /// that can occur while tmux creates or attaches the private presentation.
    /// Only an incomplete topology is retryable; all other errors, including a
    /// persistent topology failure, remain fail-closed.
    pub(crate) fn retry_default_navigator_width(&self) -> Result<(), PresentationError> {
        retry_default_navigator_width(|| self.set_default_navigator_width())
    }

    pub(super) fn default_navigator_resize_arguments_for(&self, navigator: &str) -> Vec<OsString> {
        vec![
            "resize-pane".into(),
            "-t".into(),
            self.pane_target(navigator).into(),
            "-x".into(),
            DEFAULT_NAVIGATOR_PANE_WIDTH.to_string().into(),
        ]
    }

    /// Keeps the compact split invariant at the private tmux event boundary.
    /// A detached server starts at its configured default size; when the first
    /// real client attaches, tmux otherwise expands both panes proportionally
    /// before the Navigator can receive a terminal resize event.
    pub(crate) fn install_navigator_width_hooks(&self) -> Result<(), PresentationError> {
        let navigator = self.navigator_target()?;
        for hook in NAVIGATOR_WIDTH_HOOKS {
            self.invoke(
                None,
                self.navigator_width_hook_arguments_for(hook, &navigator),
            )?;
        }
        Ok(())
    }

    pub(super) fn navigator_width_hook_arguments_for(
        &self,
        hook: &str,
        navigator: &str,
    ) -> Vec<OsString> {
        vec![
            "set-hook".into(),
            "-t".into(),
            self.paths.session_name.clone().into(),
            hook.into(),
            format!(
                "resize-pane -t {} -x {DEFAULT_NAVIGATOR_PANE_WIDTH}",
                self.pane_target(navigator)
            )
            .into(),
        ]
    }

    pub(crate) fn invoke(
        &self,
        config: Option<&Path>,
        arguments: Vec<OsString>,
    ) -> Result<(), PresentationError> {
        self.invoke_capture(config, arguments).map(|_| ())
    }

    pub(crate) fn invoke_capture(
        &self,
        config: Option<&Path>,
        arguments: Vec<OsString>,
    ) -> Result<String, PresentationError> {
        let mut command = private_tmux_command();
        if let Some(config) = config {
            command.arg("-f").arg(config);
        }
        command.arg("-S").arg(&self.paths.socket).args(arguments);
        let output = output_bounded(&mut command, MAX_TMUX_OUTPUT_BYTES, MAX_TMUX_OUTPUT_BYTES)
            .map_err(PresentationError::from_bounded_tmux)?;
        if output.status.success() {
            String::from_utf8(output.stdout).map_err(|_| {
                PresentationError::TmuxRejected(
                    "private presentation tmux output was not UTF-8".to_owned(),
                )
            })
        } else {
            Err(PresentationError::TmuxRejected(sanitize_diagnostic(
                &String::from_utf8_lossy(&output.stderr),
            )))
        }
    }
}

/// Retries only the topology observation that can be transient during a
/// private presentation attach.  The bounded policy is shared by startup and
/// the Navigator's post-attach resize path so the two entry points cannot
/// drift in their failure behavior.
pub(crate) fn retry_default_navigator_width(
    mut resize: impl FnMut() -> Result<(), PresentationError>,
) -> Result<(), PresentationError> {
    for attempt in 0..super::NAVIGATOR_WIDTH_RETRY_ATTEMPTS {
        match resize() {
            Ok(()) => return Ok(()),
            Err(error) if !matches!(error, PresentationError::InvalidTopology) => {
                return Err(error);
            }
            Err(error) if attempt + 1 == super::NAVIGATOR_WIDTH_RETRY_ATTEMPTS => {
                return Err(error);
            }
            Err(_) => std::thread::sleep(super::NAVIGATOR_WIDTH_RETRY_INTERVAL),
        }
    }
    unreachable!("navigator width retry loop has at least one attempt")
}

impl Presentation {
    /// Returns the exact owned role for a pane supplied by tmux's format
    /// expansion. No positional pane index is accepted at this boundary.
    ///
    /// # Errors
    ///
    /// Returns an error when the private pane topology is missing, dead, or
    /// ambiguous, or when the source pane is not an exact owned pane.
    pub fn focused_pane_role(
        &self,
        source_pane: &str,
    ) -> Result<PresentationPaneRole, PresentationError> {
        let topology = self.read_topology()?;
        topology
            .pane(source_pane)
            .map(|pane| pane.role)
            .ok_or(PresentationError::InvalidTopology)
    }

    /// Proves that the tmux-expanded source is the currently active provider
    /// pane in the exact fixed two-pane presentation.
    pub(crate) fn validate_focused_provider(
        &self,
        source_pane: &str,
    ) -> Result<(), PresentationError> {
        let topology = self.read_topology()?;
        let provider = topology
            .provider()
            .filter(|_| topology.panes.len() == 2 && topology.utility().is_none())
            .ok_or(PresentationError::InvalidTopology)?;
        if provider.id != source_pane {
            return Err(PresentationError::ControlRefused(
                "provider switch requires the focused provider pane",
            ));
        }
        let active = self.invoke_capture(
            None,
            vec![
                "display-message".into(),
                "-p".into(),
                "-t".into(),
                source_pane.into(),
                "#{pane_active}".into(),
            ],
        )?;
        if active.trim() == "1" {
            Ok(())
        } else {
            Err(PresentationError::ControlRefused(
                "provider switch requires the focused provider pane",
            ))
        }
    }

    /// Proves a deliberate primary-button press belongs to the exact active
    /// source and clicked target in this presentation. The caller invokes
    /// this predicate synchronously from tmux's `if-shell`; no pane is
    /// selected and no mouse event is forwarded until every check succeeds.
    pub(crate) fn validate_mouse_press(
        &self,
        target_pane: &str,
        client_name: &str,
    ) -> Result<(), PresentationError> {
        self.validate_presentation_client(client_name)?;
        let topology = self.read_topology()?;
        if topology.panes.len() != 2 || topology.utility().is_some() {
            return Err(PresentationError::ControlRefused(
                "mouse focus requires the exact two-pane presentation",
            ));
        }
        let target = topology
            .pane(target_pane)
            .ok_or(PresentationError::InvalidTopology)?;
        if !matches!(
            target.role,
            PresentationPaneRole::Navigator | PresentationPaneRole::Provider
        ) {
            return Err(PresentationError::ControlRefused(
                "mouse focus requires owned interactive panes",
            ));
        }
        let active = self.invoke_capture(
            None,
            vec![
                "display-message".into(),
                "-p".into(),
                "-c".into(),
                client_name.into(),
                "#{pane_id}|#{pane_active}".into(),
            ],
        )?;
        let mut fields = active.trim().split(TMUX_FIELD_SEPARATOR);
        let source_pane = fields.next().ok_or(PresentationError::InvalidTopology)?;
        let active_flag = fields.next().ok_or(PresentationError::InvalidTopology)?;
        if fields.next().is_some() || active_flag != "1" {
            return Err(PresentationError::ControlRefused(
                "mouse focus source is not active",
            ));
        }
        let source = topology
            .pane(source_pane)
            .ok_or(PresentationError::InvalidTopology)?;
        if !matches!(
            source.role,
            PresentationPaneRole::Navigator | PresentationPaneRole::Provider
        ) {
            return Err(PresentationError::ControlRefused(
                "mouse focus source is not interactive",
            ));
        }
        Ok(())
    }

    /// Validates that the provider role still names the exact local
    /// attachment represented by the ephemeral status row. This is called
    /// before any shell split or provider literal input.
    ///
    /// # Errors
    ///
    /// Returns an error when the private topology is ambiguous or the tagged
    /// provider context does not exactly match the supplied attachment.
    pub fn validate_provider_context(
        &self,
        workstream_id: WorkstreamId,
    ) -> Result<(), PresentationError> {
        let topology = self.read_topology()?;
        let provider = topology
            .provider()
            .ok_or(PresentationError::InvalidTopology)?;
        if provider.workstream_id != Some(workstream_id) {
            return Err(PresentationError::InvalidTopology);
        }
        Ok(())
    }

    /// Runs a bounded presentation-only action. Provider literal input is
    /// deliberately excluded: the app layer must first preflight the exact
    /// Runtime and use its private tmux socket.
    ///
    /// # Errors
    ///
    /// Returns an error when the action's source pane or owned role topology
    /// is ambiguous, or when the exact private tmux action is rejected.
    pub fn control(
        &self,
        action: PresentationAction,
        source_pane: &str,
    ) -> Result<(), PresentationError> {
        self.control_with_client(action, source_pane, None)
    }

    /// Runs one presentation action with the exact invoking tmux client.
    /// Hidden key helpers must provide the client identity so a stale or
    /// foreign client cannot mutate this private presentation.
    ///
    /// # Errors
    ///
    /// Returns an error when the action's source pane, client, or owned role
    /// topology is ambiguous, or when the exact private tmux action is
    /// rejected.
    pub fn control_with_client(
        &self,
        action: PresentationAction,
        source_pane: &str,
        client_name: Option<&str>,
    ) -> Result<(), PresentationError> {
        if let Some(client_name) = client_name {
            self.validate_presentation_client(client_name)?;
        }
        match action {
            PresentationAction::FocusLeft | PresentationAction::FocusRight => {
                self.focus_direction(source_pane, action)
            }
            PresentationAction::SwitchPrevious | PresentationAction::SwitchNext => Err(
                PresentationError::ControlRefused("provider switching requires attachment state"),
            ),
            PresentationAction::LiteralCtrlB => {
                let role = self.focused_pane_role(source_pane)?;
                if role == PresentationPaneRole::Provider {
                    return Err(PresentationError::ControlRefused(
                        "provider literal input requires Runtime preflight",
                    ));
                }
                self.send_outer_literal_c_b(source_pane)
            }
        }
    }

    /// Sends one literal C-b through the outer presentation pane. Provider
    /// panes are rejected here so they cannot accidentally invoke the nested
    /// Runtime prefix table.
    ///
    /// # Errors
    ///
    /// Returns an error when the source pane is not an exact owned non-provider
    /// pane or the private tmux server rejects the literal input.
    pub fn send_outer_literal_c_b(&self, source_pane: &str) -> Result<(), PresentationError> {
        let role = self.focused_pane_role(source_pane)?;
        if role == PresentationPaneRole::Provider {
            return Err(PresentationError::ControlRefused(
                "provider literal input requires Runtime preflight",
            ));
        }
        self.invoke(
            None,
            vec![
                "send-keys".into(),
                "-t".into(),
                source_pane.into(),
                "C-b".into(),
            ],
        )
    }

    pub(crate) fn validate_presentation_client(
        &self,
        client_name: &str,
    ) -> Result<(), PresentationError> {
        if client_name.is_empty()
            || client_name.len() > 256
            || client_name
                .chars()
                .any(|character| character.is_control() || character == TMUX_FIELD_SEPARATOR)
        {
            return Err(PresentationError::ControlRefused(
                "invoking presentation client is invalid",
            ));
        }
        let clients = self.invoke_capture(
            None,
            vec![
                "list-clients".into(),
                "-F".into(),
                "#{client_name}|#{session_name}|#{window_name}".into(),
            ],
        )?;
        if clients.lines().any(|line| {
            let mut fields = line.split(TMUX_FIELD_SEPARATOR);
            fields.next() == Some(client_name)
                && fields.next() == Some(self.paths.session_name.as_str())
                && fields.next() == Some(NAVIGATOR_WINDOW)
                && fields.next().is_none()
        }) {
            Ok(())
        } else {
            Err(PresentationError::ControlRefused(
                "invoking client is not attached to this presentation",
            ))
        }
    }

    fn focus_direction(
        &self,
        source_pane: &str,
        action: PresentationAction,
    ) -> Result<(), PresentationError> {
        let topology = self.read_topology()?;
        if topology.panes.len() != 2
            || topology.navigator().is_none()
            || topology.provider().is_none()
            || topology.utility().is_some()
        {
            return Err(PresentationError::ControlRefused(
                "focus requires the exact two-pane presentation",
            ));
        }
        let source = topology
            .pane(source_pane)
            .ok_or(PresentationError::InvalidTopology)?;
        if !matches!(
            source.role,
            PresentationPaneRole::Navigator | PresentationPaneRole::Provider
        ) {
            return Err(PresentationError::ControlRefused(
                "focus source is not an interactive presentation pane",
            ));
        }
        let target = match action {
            PresentationAction::FocusLeft => topology.directional(source, Direction::Left),
            PresentationAction::FocusRight => topology.directional(source, Direction::Right),
            _ => None,
        };
        let Some(target) = target else {
            return Err(PresentationError::ControlRefused(
                "no other owned pane in that direction",
            ));
        };
        self.select_owned_pane(&target.id)
    }

    fn select_owned_pane(&self, pane: &str) -> Result<(), PresentationError> {
        self.invoke(None, vec!["select-pane".into(), "-t".into(), pane.into()])
    }

    /// Displays one bounded guidance message in the Navigator pane.
    ///
    /// # Errors
    ///
    /// Returns an error when the exact private presentation server rejects
    /// the bounded message action.
    pub fn show_guidance(&self, message: &str) -> Result<(), PresentationError> {
        let navigator = self.navigator_target()?;
        self.invoke(
            None,
            vec![
                "display-message".into(),
                "-t".into(),
                navigator.into(),
                "-d".into(),
                "3000".into(),
                message.into(),
            ],
        )
    }

    /// Displays fixed guidance in one already-validated invoking client. The
    /// client is revalidated immediately before the tmux command and no
    /// fallback target is used if it disappeared or changed sessions.
    pub(crate) fn show_client_guidance(
        &self,
        client_name: &str,
        message: &str,
    ) -> Result<(), PresentationError> {
        self.validate_presentation_client(client_name)?;
        self.invoke(
            None,
            vec![
                "display-message".into(),
                "-c".into(),
                client_name.into(),
                "-d".into(),
                "3000".into(),
                message.into(),
            ],
        )
    }

    pub(crate) fn read_topology(&self) -> Result<PresentationTopology, PresentationError> {
        let output = self.invoke_capture(
            None,
            vec![
                "list-panes".into(),
                "-t".into(),
                format!("{}:{NAVIGATOR_WINDOW}", self.paths.session_name).into(),
                "-F".into(),
                TOPOLOGY_FORMAT.into(),
            ],
        )?;
        parse_topology(&output)
    }

    pub(crate) fn read_topology_allow_dead(
        &self,
    ) -> Result<PresentationTopology, PresentationError> {
        let output = self.invoke_capture(
            None,
            vec![
                "list-panes".into(),
                "-t".into(),
                format!("{}:{NAVIGATOR_WINDOW}", self.paths.session_name).into(),
                "-F".into(),
                TOPOLOGY_FORMAT.into(),
            ],
        )?;
        parse_topology_with_dead(&output, true)
    }

    pub(crate) fn set_pane_remain_on_exit(
        &self,
        pane: &str,
        enabled: bool,
    ) -> Result<(), PresentationError> {
        self.invoke(
            None,
            vec![
                "set-option".into(),
                "-p".into(),
                "-t".into(),
                self.pane_target(pane).into(),
                "remain-on-exit".into(),
                if enabled { "on" } else { "off" }.into(),
            ],
        )
    }

    fn try_presentation_claim(&self, token: &str) -> Result<bool, PresentationError> {
        match self.invoke(
            None,
            vec![
                "set-option".into(),
                "-g".into(),
                "-o".into(),
                PRESENTATION_CLAIM_OPTION.into(),
                token.into(),
            ],
        ) {
            Ok(()) => Ok(true),
            Err(PresentationError::TmuxRejected(message)) if message.contains("already set") => {
                Ok(false)
            }
            Err(error) => Err(error),
        }
    }

    fn release_presentation_claim(&self, token: &str) {
        let current = self.invoke_capture(
            None,
            vec![
                "show-options".into(),
                "-gqv".into(),
                PRESENTATION_CLAIM_OPTION.into(),
            ],
        );
        if current
            .ok()
            .as_deref()
            .is_some_and(|value| value.trim() == token)
        {
            let _ = self.invoke(
                None,
                vec![
                    "set-option".into(),
                    "-g".into(),
                    "-u".into(),
                    PRESENTATION_CLAIM_OPTION.into(),
                ],
            );
        }
    }

    pub(crate) fn with_attachment_claim<T>(
        &self,
        operation: impl FnOnce() -> Result<T, PresentationError>,
    ) -> Result<T, PresentationError> {
        // The presentation claim is held through provider retag/respawn so a
        // concurrent action cannot mutate the exact two-pane topology.
        let token = format!(
            "attachment-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        );
        if !self.try_presentation_claim(&token)? {
            return Err(PresentationError::ControlRefused(
                "another presentation action is in progress",
            ));
        }
        let result = operation();
        self.release_presentation_claim(&token);
        result
    }

    pub(crate) fn set_pane_role(
        &self,
        pane: &str,
        role: PresentationPaneRole,
        context: Option<WorkstreamId>,
    ) -> Result<(), PresentationError> {
        let role_name = match role {
            PresentationPaneRole::Navigator => "navigator",
            PresentationPaneRole::Provider => "provider",
            PresentationPaneRole::Utility => "utility",
        };
        let target = self.pane_target(pane);
        self.invoke(
            None,
            vec![
                "set-option".into(),
                "-p".into(),
                "-t".into(),
                target.clone().into(),
                ROLE_OPTION.into(),
                role_name.into(),
            ],
        )?;
        self.clear_pane_context(pane)?;
        if let Some(workstream_id) = context {
            self.invoke(
                None,
                vec![
                    "set-option".into(),
                    "-p".into(),
                    "-t".into(),
                    target.into(),
                    WORKSTREAM_OPTION.into(),
                    workstream_id.to_string().into(),
                ],
            )?;
        }
        Ok(())
    }

    fn clear_pane_context(&self, pane: &str) -> Result<(), PresentationError> {
        let target = self.pane_target(pane);
        self.invoke(
            None,
            vec![
                "set-option".into(),
                "-p".into(),
                "-u".into(),
                "-t".into(),
                target.into(),
                WORKSTREAM_OPTION.into(),
            ],
        )?;
        Ok(())
    }

    pub(crate) fn pane_target(&self, pane: &str) -> String {
        if pane.starts_with('%') {
            pane.to_owned()
        } else {
            format!("{}:{pane}", self.paths.session_name)
        }
    }

    fn navigator_target(&self) -> Result<String, PresentationError> {
        self.read_topology()?
            .navigator()
            .map(|pane| pane.id.clone())
            .ok_or(PresentationError::InvalidTopology)
    }

    /// Attachment replacement is the one active path that may accept an exact
    /// dead provider helper pane: tmux retains that owned pane specifically so
    /// `respawn-pane -k` can reconnect another live Runtime in place. A dead
    /// navigator remains a hard refusal, and all ordinary topology reads keep
    /// rejecting dead panes.
    pub(crate) fn attachment_topology(&self) -> Result<PresentationTopology, PresentationError> {
        let topology = self.read_topology_allow_dead()?;
        if topology.navigator().is_none_or(|pane| pane.dead) || topology.provider().is_none() {
            return Err(PresentationError::InvalidTopology);
        }
        if topology.utility().is_some() {
            return Err(PresentationError::ControlRefused(
                "presentation must remain a two-pane topology",
            ));
        }
        Ok(topology)
    }

    pub(crate) fn provider_target_for_attachment(&self) -> Result<String, PresentationError> {
        self.attachment_topology()?
            .provider()
            .map(|pane| pane.id.clone())
            .ok_or(PresentationError::InvalidTopology)
    }

    pub(crate) fn validate_provisional_attachment(
        &self,
        state: &CurrentState,
        provisional_lease: &ProvisionalLease,
        slot: &ProvisionalSlot,
    ) -> Result<(), PresentationError> {
        let unavailable =
            || PresentationError::ControlRefused("provisional shell attachment is unavailable");
        provisional_lease
            .revalidate_for_mutation(state.root())
            .map_err(|_| unavailable())?;
        if slot.phase() != ProvisionalPhase::Materialized
            || slot.lease_generation() != provisional_lease.lease_generation()
        {
            return Err(unavailable());
        }
        let context = Self::context_from_directory(state.root(), &self.paths.directory)
            .map_err(|_| unavailable())?;
        if slot.presentation_id() != context.presentation_id()
            || slot.presentation_revision() != context.presentation_revision()
            || slot.seed_cwd() != context.seed_cwd()
        {
            return Err(unavailable());
        }
        if read_marker(state.root(), &self.paths.directory).map_err(|_| unavailable())? != *slot {
            return Err(unavailable());
        }
        provisional_lease
            .revalidate_for_mutation(state.root())
            .map_err(|_| unavailable())
    }

    pub(crate) fn install_control_bindings(&self) -> Result<(), PresentationError> {
        // Reconciliation is a mutating tmux boundary. Prove the owned
        // context and allowed two-pane topology before changing even a
        // presentation option or key table.
        self.context()?;
        self.attachment_topology()?;
        for (option, value) in [
            ("status", "off"),
            ("mouse", "on"),
            ("remain-on-exit", "on"),
            ("prefix", "C-b"),
            ("prefix2", "None"),
            ("pane-border-status", "top"),
            (
                "pane-border-format",
                " #{?pane_active,▶ ACTIVE,◇ INACTIVE} ",
            ),
        ] {
            self.invoke(
                None,
                vec![
                    "set-option".into(),
                    "-g".into(),
                    option.into(),
                    value.into(),
                ],
            )?;
        }
        for table in ["prefix", "root"] {
            self.reset_key_table(table)?;
        }
        let bindings = [
            ("Up", PresentationAction::SwitchPrevious),
            ("Down", PresentationAction::SwitchNext),
            ("Left", PresentationAction::FocusLeft),
            ("Right", PresentationAction::FocusRight),
            ("C-b", PresentationAction::LiteralCtrlB),
        ];
        for (key, action) in bindings {
            // Deliberately omit `-b`: tmux waits for this fixed helper before
            // accepting another key action, which makes focus requests
            // serialize without a lock that could outlive a failed helper.
            self.invoke(
                None,
                vec![
                    "bind-key".into(),
                    "-T".into(),
                    "prefix".into(),
                    key.into(),
                    "run-shell".into(),
                    self.control_shell_command(action)?.into(),
                ],
            )?;
        }
        self.invoke(
            None,
            vec![
                "bind-key".into(),
                "-T".into(),
                "prefix".into(),
                "d".into(),
                "detach-client".into(),
            ],
        )?;
        self.invoke(
            None,
            vec![
                "bind-key".into(),
                "-T".into(),
                "prefix".into(),
                "?".into(),
                "display-message".into(),
                "Ctrl+b: Left/Right focus | Up/Down switch | d detach | Ctrl+b literal | ? help"
                    .into(),
            ],
        )?;
        self.install_root_mouse_bindings()
    }

    fn install_root_mouse_bindings(&self) -> Result<(), PresentationError> {
        let mouse_validation = self.mouse_press_shell_command()?;
        let bindings = [
            vec![
                "MouseDown1Pane",
                "if-shell",
                mouse_validation.as_str(),
                "select-pane -t = ; send-keys -M",
            ],
            vec!["MouseUp1Pane", "send-keys", "-M"],
            vec![
                "MouseDrag1Pane",
                "if-shell",
                "-F",
                "#{||:#{pane_in_mode},#{mouse_any_flag}}",
                "send-keys -M",
                "copy-mode -M",
            ],
            vec![
                "WheelUpPane",
                "if-shell",
                "-F",
                "#{||:#{alternate_on},#{pane_in_mode},#{mouse_any_flag}}",
                "send-keys -M",
                "copy-mode -e",
            ],
            vec![
                "WheelDownPane",
                "if-shell",
                "-F",
                "#{||:#{alternate_on},#{pane_in_mode},#{mouse_any_flag}}",
                "send-keys -M",
                "send-keys -M",
            ],
        ];
        for binding in bindings {
            let mut arguments = vec![
                OsString::from("bind-key"),
                OsString::from("-T"),
                OsString::from("root"),
            ];
            arguments.extend(binding.into_iter().map(OsString::from));
            self.invoke(None, arguments)?;
        }
        for binding in COPY_MODE_SCROLL_BINDINGS {
            self.invoke(
                None,
                binding
                    .arguments()
                    .into_iter()
                    .map(OsString::from)
                    .collect(),
            )?;
        }
        Ok(())
    }

    /// Tmux creates a key table lazily. Seed it with a fixed, immediately
    /// removed binding before clearing so reconciliation never interprets a
    /// missing-table diagnostic permissively.
    fn reset_key_table(&self, table: &str) -> Result<(), PresentationError> {
        self.invoke(
            None,
            vec![
                "bind-key".into(),
                "-T".into(),
                table.into(),
                "F12".into(),
                "display-message".into(),
                "".into(),
            ],
        )?;
        self.invoke(
            None,
            vec!["unbind-key".into(), "-a".into(), "-T".into(), table.into()],
        )
    }

    pub(super) fn control_shell_command(
        &self,
        action: PresentationAction,
    ) -> Result<String, PresentationError> {
        let executable = shell_quote(self.executable.as_os_str())?;
        let state_root = shell_quote(self.state_root.as_os_str())?;
        let socket = shell_quote(self.paths.socket.as_os_str())?;
        let session = shell_quote(self.paths.session_name.as_ref())?;
        Ok(format!(
            "exec {executable} --state-root {state_root} _presentation_control --presentation-socket {socket} --presentation-session {session} --action {} --source-pane '#{{pane_id}}' --client-name #{{q:client_name}}",
            action.as_str()
        ))
    }

    pub(super) fn mouse_press_shell_command(&self) -> Result<String, PresentationError> {
        super::presentation_mouse_validation_command(
            &self.paths,
            &self.executable,
            &self.state_root,
        )
    }
}
