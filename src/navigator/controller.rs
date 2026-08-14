use std::{
    io::{self, stdout},
    path::PathBuf,
    process::Command,
    str::FromStr,
    time::{Duration, Instant},
};

use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, MouseButton, MouseEventKind,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};

use crate::{
    domain::{ProjectId, ProviderKind, WorkstreamId},
    presentation::{Presentation, PresentationError},
    process::output_bounded,
    protocol::{HostAction, ObserverStatus, ProjectDirectoriesResponse, ProviderCapability},
    state::{
        ClientCatalog, ClientHostTransport, HostRegistry, IntegrationLifecycle, StateError,
        StateRoot,
    },
    transport::{
        HostClient, LocalEndpoint, RemoteExecutable, SshDestination, SshEndpoint,
        SystemCommandRunner,
    },
};

use super::{
    model::provider_label,
    model::{
        LocalNavigatorSnapshot, NavigatorError, NavigatorHost, NavigatorOperation,
        NavigatorRuntimeStatus, NavigatorWorkstream, RemoteHostReachability,
    },
    snapshot::{MAX_NAVIGATOR_TEXT_INPUT_BYTES, RemoteMonitor, bounded_display, combined_snapshot},
    view::{
        MouseClickIntent, NavigatorDetail, NavigatorModal, NavigatorPage, NavigatorView,
        ProviderChoice, ProviderChoiceIntent,
    },
};

/// Runs the internal Ratatui process inside one owned presentation pane.
/// The presentation owner supplies the only private tmux socket this process
/// may mutate.
///
/// # Errors
///
/// Returns an error when the local terminal cannot be initialized, the private
/// presentation control path is invalid, or bounded local state/action calls
/// fail.
pub fn run_local_navigator(
    root: &StateRoot,
    socket: PathBuf,
    session_name: String,
) -> Result<(), NavigatorError> {
    let presentation = Presentation::from_control(root.base(), socket, session_name)?;
    let mut remote = RemoteMonitor::new();
    remote.set_installation_cache(crate::provider::InstallationProbeCache::probe());
    let snapshot = combined_snapshot(root, &mut remote, None)?;
    let mut view = NavigatorView::new(snapshot);
    let mut observer_needs_review = initialize_observer_activation_message(root, &mut view);
    let mut terminal = TerminalSession::enter()?;
    let mut last_refresh = Instant::now();
    let mut needs_redraw = true;
    needs_redraw |= refresh_attachment_status(&presentation, &mut view);
    let outcome: Result<(), NavigatorError> = loop {
        if needs_redraw {
            terminal.terminal.draw(|frame| view.render(frame))?;
            needs_redraw = false;
        }
        let timeout = Duration::from_millis(100);
        if event::poll(timeout)? {
            let (exit, event_changed) = match event::read()? {
                Event::Key(key) => (
                    handle_navigator_key(key, root, &presentation, &mut remote, &mut view),
                    true,
                ),
                Event::Mouse(mouse) if !view.help_visible() => {
                    handle_navigator_mouse(mouse, root, &presentation, &mut remote, &mut view);
                    (false, true)
                }
                Event::Resize(_, _) => {
                    if let Err(error) = presentation.set_default_navigator_width() {
                        view.set_message(action_message(&error));
                    }
                    (false, true)
                }
                _ => (false, false),
            };
            if exit {
                break Ok(());
            }
            needs_redraw |= event_changed;
        }
        if last_refresh.elapsed() >= Duration::from_millis(500) {
            needs_redraw |= refresh_navigator(
                root,
                &presentation,
                &mut remote,
                &mut view,
                &mut observer_needs_review,
            );
            last_refresh = Instant::now();
        }
        if view.expire_transient_message(Instant::now()) {
            needs_redraw = true;
        }
    };
    drop(terminal);
    let close = presentation.close();
    outcome?;
    close?;
    Ok(())
}

fn handle_navigator_key(
    key: crossterm::event::KeyEvent,
    root: &StateRoot,
    presentation: &Presentation,
    remote: &mut RemoteMonitor,
    view: &mut NavigatorView,
) -> bool {
    if view.modal_visible() {
        return handle_navigator_modal_key(key, root, presentation, remote, view);
    }
    if view.help_visible() {
        match key.code {
            KeyCode::Char('?' | 'q') | KeyCode::Esc => view.dismiss_help(),
            KeyCode::Down | KeyCode::Char('j') => view.scroll_help_next(),
            KeyCode::Up | KeyCode::Char('k') => view.scroll_help_previous(),
            _ => {}
        }
        return false;
    }
    if matches!(view.detail, Some(NavigatorDetail::ForkRecovery { .. }))
        && matches!(key.code, KeyCode::Enter)
    {
        recover_selected_operation(root, presentation, remote, view);
        return false;
    }
    let workstreams = view.page() == NavigatorPage::Workstreams && view.detail.is_none();
    if workstreams && handle_workstream_action_key(key.code, root, presentation, remote, view) {
        return false;
    }
    if !workstreams
        && view.detail.is_none()
        && handle_management_page_key(key.code, root, presentation, remote, view)
    {
        return false;
    }
    match key.code {
        KeyCode::Char('q') => true,
        KeyCode::Esc => {
            if view.dismiss_detail() {
                false
            } else if view.page() != NavigatorPage::Workstreams {
                view.select_page(NavigatorPage::Workstreams);
                false
            } else {
                true
            }
        }
        KeyCode::Char('?') => {
            view.toggle_help();
            false
        }
        KeyCode::Char(',') => {
            view.toggle_management_page(NavigatorPage::Projects);
            false
        }
        KeyCode::Char('.') => {
            view.toggle_management_page(NavigatorPage::Hosts);
            false
        }
        KeyCode::Right if workstreams => {
            view.cycle_view_mode_next();
            false
        }
        KeyCode::Left if workstreams => {
            view.cycle_view_mode_previous();
            false
        }
        KeyCode::Down | KeyCode::Char('j') => {
            view.select_next();
            false
        }
        KeyCode::Up | KeyCode::Char('k') => {
            view.select_previous();
            false
        }
        KeyCode::Tab if workstreams && !view.view_mode.is_archived() => {
            if let Err(error) = presentation.focus_provider() {
                view.set_message(action_message(&error));
            }
            false
        }
        KeyCode::Enter if workstreams => {
            activate_selected(
                root,
                presentation,
                remote,
                view,
                WorkstreamActivationInput::Enter,
            );
            false
        }
        KeyCode::Char('i') if workstreams => {
            view.open_selected_detail();
            false
        }
        _ => false,
    }
}

fn handle_management_page_key(
    key: KeyCode,
    root: &StateRoot,
    presentation: &Presentation,
    remote: &mut RemoteMonitor,
    view: &mut NavigatorView,
) -> bool {
    match (view.page(), key) {
        (NavigatorPage::Projects, KeyCode::Char('a')) => view.begin_checkout_registration(),
        (NavigatorPage::Projects, KeyCode::Char('x')) => view.begin_project_forget(),
        (NavigatorPage::Hosts, KeyCode::Char('a')) => view.begin_host_registration(),
        (NavigatorPage::Hosts, KeyCode::Char('s')) => {
            activate_selected_host(root, presentation, remote, view);
        }
        (NavigatorPage::Hosts, KeyCode::Char('r')) => {
            view.begin_project_browser_root_configuration();
        }
        (NavigatorPage::Hosts, KeyCode::Char('x')) => forget_selected_host(view),
        _ => return false,
    }
    true
}

fn handle_workstream_action_key(
    key: KeyCode,
    root: &StateRoot,
    presentation: &Presentation,
    remote: &mut RemoteMonitor,
    view: &mut NavigatorView,
) -> bool {
    if view.view_mode.is_archived() {
        if key == KeyCode::Char('u') {
            restore_selected(root, remote, view);
            return true;
        }
        return false;
    }
    match key {
        KeyCode::Char('a') => acknowledge_selected(root, remote, view),
        KeyCode::Char('p') => park_selected(root, remote, view),
        KeyCode::Char('x') => archive_selected(root, remote, view),
        KeyCode::Char('r') => rename_selected(view),
        KeyCode::Char('n') => {
            create_workstream_selected(
                root,
                presentation,
                remote,
                view,
                CreationAction::Independent,
            );
        }
        KeyCode::Char('f') => {
            create_workstream_selected(root, presentation, remote, view, CreationAction::Fork);
        }
        _ => return false,
    }
    true
}

fn handle_navigator_modal_key(
    key: crossterm::event::KeyEvent,
    root: &StateRoot,
    presentation: &Presentation,
    remote: &mut RemoteMonitor,
    view: &mut NavigatorView,
) -> bool {
    if matches!(view.modal, Some(NavigatorModal::ProjectBrowser { .. })) {
        return handle_project_browser_modal_key(key, root, remote, view);
    }
    if matches!(key.code, KeyCode::Esc) {
        view.dismiss_modal();
        return false;
    }
    if matches!(key.code, KeyCode::Char('n'))
        && matches!(view.modal, Some(NavigatorModal::ConfirmForkRecovery { .. }))
    {
        if let Some(NavigatorModal::ConfirmForkRecovery { source, .. }) = view.confirm_modal() {
            create_workstream_from_source(
                root,
                presentation,
                remote,
                view,
                &source,
                CreationAction::Fork,
                false,
                None,
            );
        }
        return false;
    }
    if matches!(key.code, KeyCode::Char('n')) && is_confirmation_modal(view.modal.as_ref()) {
        view.dismiss_modal();
        return false;
    }
    if handle_modal_picker_key(key.code, view) {
        return false;
    }
    if matches!(key.code, KeyCode::Enter) {
        confirm_navigator_modal(root, presentation, remote, view);
        return false;
    }
    if matches!(key.code, KeyCode::Char('y')) && is_confirmation_modal(view.modal.as_ref()) {
        confirm_navigator_modal(root, presentation, remote, view);
        return false;
    }
    match view.modal.as_mut() {
        Some(NavigatorModal::Rename { value, .. }) => match key.code {
            KeyCode::Backspace => {
                value.pop();
            }
            KeyCode::Char(character) if !character.is_control() && value.chars().count() < 512 => {
                value.push(character);
            }
            _ => {}
        },
        Some(NavigatorModal::ConfigureProjectBrowserRoot { value, .. }) => match key.code {
            KeyCode::Backspace => {
                value.pop();
            }
            KeyCode::Char(character)
                if !character.is_control() && value.len() < MAX_NAVIGATOR_TEXT_INPUT_BYTES =>
            {
                value.push(character);
            }
            _ => {}
        },
        Some(NavigatorModal::RegisterHost { value }) => match key.code {
            KeyCode::Backspace => {
                value.pop();
            }
            KeyCode::Char(character) if !character.is_control() && value.len() < 255 => {
                value.push(character);
            }
            _ => {}
        },
        Some(
            NavigatorModal::ConfirmArchive(_)
            | NavigatorModal::ConfirmForkRecovery { .. }
            | NavigatorModal::SelectHostRemoval { .. }
            | NavigatorModal::ConfirmForgetProject { .. }
            | NavigatorModal::SelectRegistrationHost { .. }
            | NavigatorModal::SelectProvider { .. }
            | NavigatorModal::ProjectBrowser { .. },
        )
        | None => {}
    }
    false
}

fn handle_project_browser_modal_key(
    key: crossterm::event::KeyEvent,
    root: &StateRoot,
    remote: &mut RemoteMonitor,
    view: &mut NavigatorView,
) -> bool {
    match key.code {
        KeyCode::Esc => view.dismiss_modal(),
        KeyCode::Down | KeyCode::Char('j') => view.select_project_browser_next(),
        KeyCode::Up | KeyCode::Char('k') => view.select_project_browser_previous(),
        KeyCode::Backspace => {
            if let Some(NavigatorModal::ProjectBrowser { filter, .. }) = view.modal.as_mut() {
                filter.pop();
            }
            view.normalize_project_browser_selection();
        }
        KeyCode::Char('h') => {
            let Some((host, cursor)) = view.project_browser_cursor() else {
                return false;
            };
            let parent = cursor
                .rsplit_once('/')
                .map_or_else(String::new, |(parent, _)| parent.to_owned());
            open_project_browser(root, view, host, &parent);
        }
        KeyCode::Char('r') => {
            let Some((host, cursor)) = view.project_browser_cursor() else {
                return false;
            };
            register_project_browser_directory(root, remote, view, &host, &cursor);
        }
        KeyCode::Enter => {
            let Some((host, cursor, entry)) = view.project_browser_selected_entry() else {
                view.set_message("no folder is selected");
                return false;
            };
            let relative_path = if cursor.is_empty() {
                entry.name
            } else {
                format!("{cursor}/{}", entry.name)
            };
            if entry.is_git_repository {
                register_project_browser_directory(root, remote, view, &host, &relative_path);
            } else {
                open_project_browser(root, view, host, &relative_path);
            }
        }
        KeyCode::Char(character) if !character.is_control() => {
            if let Some(NavigatorModal::ProjectBrowser { filter, .. }) = view.modal.as_mut()
                && filter.chars().count() < 64
            {
                filter.push(character);
            }
            view.normalize_project_browser_selection();
        }
        _ => {}
    }
    false
}

fn handle_modal_picker_key(key: KeyCode, view: &mut NavigatorView) -> bool {
    let registration_host = matches!(
        view.modal,
        Some(NavigatorModal::SelectRegistrationHost { .. })
    );
    let host_removal = matches!(view.modal, Some(NavigatorModal::SelectHostRemoval { .. }));
    let provider = matches!(view.modal, Some(NavigatorModal::SelectProvider { .. }));
    if !registration_host && !host_removal && !provider {
        return false;
    }
    match key {
        KeyCode::Down | KeyCode::Char('j') => {
            if registration_host {
                view.select_registration_host_next();
            } else if provider {
                view.select_provider_next();
            } else {
                view.toggle_host_removal_mode();
            }
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if registration_host {
                view.select_registration_host_previous();
            } else if provider {
                view.select_provider_previous();
            } else {
                view.toggle_host_removal_mode();
            }
        }
        KeyCode::Enter => return false,
        _ => {}
    }
    true
}

fn is_confirmation_modal(modal: Option<&NavigatorModal>) -> bool {
    matches!(
        modal,
        Some(
            NavigatorModal::ConfirmArchive(_)
                | NavigatorModal::ConfirmForkRecovery { .. }
                | NavigatorModal::ConfirmForgetProject { .. }
        )
    )
}

fn confirm_navigator_modal(
    root: &StateRoot,
    presentation: &Presentation,
    remote: &mut RemoteMonitor,
    view: &mut NavigatorView,
) {
    match view.confirm_modal() {
        Some(NavigatorModal::ConfirmArchive(workstream)) => {
            archive_workstream(root, remote, view, &workstream);
        }
        Some(NavigatorModal::ConfirmForkRecovery { operation, .. }) => {
            recover_operation(root, presentation, remote, view, &operation);
        }
        Some(removal @ NavigatorModal::SelectHostRemoval { .. }) => {
            confirm_host_removal(root, remote, view, removal);
        }
        Some(NavigatorModal::ConfirmForgetProject {
            project_id,
            label,
            archived_workstream_count,
            location_count,
        }) => forget_project(
            root,
            remote,
            view,
            project_id,
            &label,
            archived_workstream_count,
            location_count,
        ),
        Some(NavigatorModal::Rename { workstream, value }) => {
            rename_workstream(root, remote, view, &workstream, &value);
        }
        Some(NavigatorModal::SelectRegistrationHost { hosts, selected }) => {
            if let Some(host) = hosts.get(selected).cloned() {
                open_project_browser(root, view, host, "");
            } else {
                view.set_message("no registered host is available for Project registration");
            }
        }
        Some(NavigatorModal::SelectProvider {
            providers,
            selected,
            intent,
        }) => {
            let Some(provider) = providers.get(selected).copied() else {
                view.set_message("no provider is eligible for a new Workstream");
                return;
            };
            let host = match &intent {
                ProviderChoiceIntent::New { source } => &source.host,
                ProviderChoiceIntent::Register { host, .. } => host,
            };
            if !view.provider_choice_is_current(host, provider) {
                view.set_message("provider choice is no longer available; refresh and try again");
                return;
            }
            match intent {
                ProviderChoiceIntent::New { source } => create_workstream_from_source(
                    root,
                    presentation,
                    remote,
                    view,
                    &source,
                    CreationAction::Independent,
                    true,
                    Some(provider),
                ),
                ProviderChoiceIntent::Register {
                    host,
                    relative_path,
                } => register_project_browser_directory_with_provider(
                    root,
                    remote,
                    view,
                    &host,
                    &relative_path,
                    provider,
                ),
            }
        }
        Some(NavigatorModal::ConfigureProjectBrowserRoot { host, value })
            if value.trim().is_empty() =>
        {
            view.modal = Some(NavigatorModal::ConfigureProjectBrowserRoot { host, value });
            view.set_message("enter a host-local project browser root");
        }
        Some(NavigatorModal::ConfigureProjectBrowserRoot { host, value }) => {
            configure_project_browser_root(root, view, &host, &value);
        }
        Some(NavigatorModal::ProjectBrowser { .. }) | None => {}
        Some(NavigatorModal::RegisterHost { value }) if value.trim().is_empty() => {
            view.modal = Some(NavigatorModal::RegisterHost { value });
            view.set_message("enter an SSH destination to register");
        }
        Some(NavigatorModal::RegisterHost { value }) => {
            register_remote_host(root, presentation, remote, view, &value);
        }
    }
}

fn confirm_host_removal(
    root: &StateRoot,
    remote: &mut RemoteMonitor,
    view: &mut NavigatorView,
    removal: NavigatorModal,
) {
    let NavigatorModal::SelectHostRemoval {
        alias,
        workstream_count,
        location_count,
        unresolved_operation_count,
        offboard,
    } = removal
    else {
        unreachable!("host-removal action requires a host-removal modal");
    };
    if offboard {
        offboard_host(
            root,
            remote,
            view,
            &alias,
            workstream_count,
            location_count,
            unresolved_operation_count,
        );
    } else {
        forget_host(
            root,
            remote,
            view,
            &alias,
            workstream_count,
            location_count,
            unresolved_operation_count,
        );
    }
}

fn handle_navigator_mouse(
    mouse: crossterm::event::MouseEvent,
    root: &StateRoot,
    presentation: &Presentation,
    remote: &mut RemoteMonitor,
    view: &mut NavigatorView,
) {
    if view.modal_visible() {
        return;
    }
    match mouse.kind {
        MouseEventKind::ScrollDown => view.select_next(),
        MouseEventKind::ScrollUp => view.select_previous(),
        MouseEventKind::Down(MouseButton::Left) => {
            if view.detail.is_some() {
                view.begin_mouse_click(None);
            } else {
                match view.page() {
                    NavigatorPage::Workstreams => {
                        view.begin_mouse_click(view.row_from_y(mouse.row));
                    }
                    NavigatorPage::Projects => {
                        view.begin_project_click(view.project_from_y(mouse.row));
                    }
                    NavigatorPage::Hosts => view.begin_host_click(view.host_from_y(mouse.row)),
                }
            }
        }
        MouseEventKind::Up(MouseButton::Left) => match view.take_mouse_click() {
            Some(MouseClickIntent::Row) => activate_selected(
                root,
                presentation,
                remote,
                view,
                WorkstreamActivationInput::MouseClick,
            ),
            Some(MouseClickIntent::Management | MouseClickIntent::Blank) => {
                if let Err(error) = presentation.focus_navigator() {
                    view.set_message(action_message(&error));
                }
            }
            None => {}
        },
        _ => {}
    }
}

fn initialize_observer_activation_message(root: &StateRoot, view: &mut NavigatorView) -> bool {
    let pending = observer_review_pending(root, local_provider_capabilities(&view.snapshot));
    if pending {
        view.set_message(
            "approve the observer hooks in the native Codex pane with /hooks, then exit Codex",
        );
    }
    pending
}

fn refresh_navigator(
    root: &StateRoot,
    presentation: &Presentation,
    remote: &mut RemoteMonitor,
    view: &mut NavigatorView,
    observer_needs_review: &mut bool,
) -> bool {
    let mut changed = false;
    let selected_host = view.selected_host_alias().map(str::to_owned);
    let now_pending = match combined_snapshot(root, remote, selected_host.as_deref()) {
        Ok(snapshot) => {
            let now_pending = observer_review_pending(root, local_provider_capabilities(&snapshot));
            changed |= view.replace_snapshot(snapshot);
            now_pending
        }
        Err(error) => {
            view.set_message(action_message(&error));
            changed = true;
            observer_review_pending(root, local_provider_capabilities(&view.snapshot))
        }
    };
    changed |= refresh_attachment_status(presentation, view);
    if *observer_needs_review && !now_pending {
        view.set_message("observer ready; native Workstreams can now start");
        changed = true;
    }
    *observer_needs_review = now_pending;
    changed
}

fn local_provider_capabilities(snapshot: &LocalNavigatorSnapshot) -> &[ProviderCapability] {
    snapshot
        .hosts
        .iter()
        .find(|host| host.alias == "local")
        .map_or(&[], |host| host.provider_capabilities.as_slice())
}

fn observer_review_pending(root: &StateRoot, capabilities: &[ProviderCapability]) -> bool {
    let Ok(registry) = HostRegistry::open(root) else {
        return false;
    };
    let opencode_eligible = capabilities
        .iter()
        .find(|capability| capability.kind == ProviderKind::OpenCode)
        .is_some_and(|capability| capability.is_new_eligible());
    let codex_eligible = capabilities
        .iter()
        .find(|capability| capability.kind == ProviderKind::Codex)
        .is_some_and(|capability| capability.is_new_eligible());
    if opencode_eligible && !codex_eligible {
        return false;
    }
    registry
        .codex_integration()
        .ok()
        .flatten()
        .is_none_or(|integration| integration.lifecycle != IntegrationLifecycle::Ready)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::navigator) enum PostActivationFocus {
    Provider,
    Navigator,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::navigator) enum WorkstreamActivationInput {
    Enter,
    MouseClick,
    ProviderHandoff,
}

impl WorkstreamActivationInput {
    const fn post_activation_focus(self) -> PostActivationFocus {
        match self {
            Self::Enter | Self::ProviderHandoff => PostActivationFocus::Provider,
            Self::MouseClick => PostActivationFocus::Navigator,
        }
    }
}

/// Narrow focus-only seam for deterministic controller tests. Presentation
/// remains the sole production implementation; activation and attachment
/// behavior stay on the concrete presentation owner.
pub(in crate::navigator) trait NavigatorFocus {
    fn focus_provider(&self) -> Result<(), PresentationError>;
    fn focus_navigator(&self) -> Result<(), PresentationError>;
}

impl NavigatorFocus for Presentation {
    fn focus_provider(&self) -> Result<(), PresentationError> {
        Presentation::focus_provider(self)
    }

    fn focus_navigator(&self) -> Result<(), PresentationError> {
        Presentation::focus_navigator(self)
    }
}

pub(in crate::navigator) fn apply_post_activation_focus<F: NavigatorFocus>(
    focus: &F,
    policy: PostActivationFocus,
) -> Result<(), PresentationError> {
    match policy {
        PostActivationFocus::Provider => focus.focus_provider(),
        PostActivationFocus::Navigator => focus.focus_navigator(),
    }
}

pub(in crate::navigator) fn focus_if_already_attached<F: NavigatorFocus>(
    focus: &F,
    view: &mut NavigatorView,
    selected: &NavigatorWorkstream,
    input: WorkstreamActivationInput,
) -> bool {
    if !view.is_attached_to(selected)
        || matches!(
            selected.runtime_status,
            NavigatorRuntimeStatus::Parked | NavigatorRuntimeStatus::Unknown
        )
    {
        return false;
    }
    if let Err(error) = apply_post_activation_focus(focus, input.post_activation_focus()) {
        view.set_message(action_message(&error));
    }
    true
}

fn activate_selected(
    root: &StateRoot,
    presentation: &Presentation,
    remote: &mut RemoteMonitor,
    view: &mut NavigatorView,
    input: WorkstreamActivationInput,
) {
    let Some(selected) = view.selected().cloned() else {
        view.set_message("no Workstream is registered; add a Git Project first");
        return;
    };
    if selected.archived {
        view.set_message(format!(
            "restore this Workstream before opening {}",
            provider_label(selected.provider)
        ));
        return;
    }
    if selected.host.is_remote() && !selected.host.is_reachable() {
        view.set_message("remote host is unavailable; cached state is not actionable");
        return;
    }
    if focus_if_already_attached(presentation, view, &selected, input) {
        return;
    }
    let lifecycle_action = match selected.runtime_status {
        NavigatorRuntimeStatus::Parked | NavigatorRuntimeStatus::Unknown => Some("start"),
        NavigatorRuntimeStatus::RecoveryRequired => Some("recover"),
        NavigatorRuntimeStatus::Starting
        | NavigatorRuntimeStatus::Idle
        | NavigatorRuntimeStatus::Working
        | NavigatorRuntimeStatus::Attention => None,
    };
    if let Some(action) = lifecycle_action
        && let Err(error) = run_action(root, action, &selected, None)
    {
        view.set_message(action_message(&error));
        return;
    }
    remote.request_soon(selected.host.alias());
    refresh_view(root, remote, view);
    let attachment = if selected.host.is_remote() {
        presentation.attach_remote_workstream(selected.host.alias(), selected.workstream_id)
    } else {
        presentation.attach_workstream(selected.workstream_id)
    };
    let attachment = match attachment {
        Ok(attachment) => attachment,
        Err(error) => {
            view.set_message(action_message(&error));
            return;
        }
    };
    view.observe_attachment(&attachment);
    if let Err(error) = apply_post_activation_focus(presentation, input.post_activation_focus()) {
        view.set_message(action_message(&error));
    }
}

fn acknowledge_selected(root: &StateRoot, remote: &mut RemoteMonitor, view: &mut NavigatorView) {
    let Some(selected) = view.selected().cloned() else {
        return;
    };
    let Some(revision) = selected.attention_revision else {
        view.set_message("no result or recovery attention to acknowledge");
        return;
    };
    match run_action(root, "acknowledge", &selected, Some(revision.value())) {
        Ok(()) => {
            remote.request_soon(selected.host.alias());
            refresh_view(root, remote, view);
            view.set_message("attention acknowledged");
        }
        Err(error) => view.set_message(action_message(&error)),
    }
}

fn park_selected(root: &StateRoot, remote: &mut RemoteMonitor, view: &mut NavigatorView) {
    let Some(selected) = view.selected().cloned() else {
        return;
    };
    match run_action(root, "park", &selected, None) {
        Ok(()) => {
            view.clear_attached(&selected);
            remote.request_soon(selected.host.alias());
            refresh_view(root, remote, view);
            view.set_message("Workstream parked; provider history is preserved");
        }
        Err(error) => view.set_message(action_message(&error)),
    }
}

fn archive_selected(root: &StateRoot, remote: &mut RemoteMonitor, view: &mut NavigatorView) {
    let Some(selected) = view.selected().cloned() else {
        view.set_message("no active Workstream is selected");
        return;
    };
    if selected.archived {
        view.set_message("this Workstream is already archived");
        return;
    }
    if selected.host.is_remote() && !selected.host.is_reachable() {
        view.set_message("remote host is unavailable; cached state is not actionable");
        return;
    }
    if selected.runtime_status == NavigatorRuntimeStatus::Working {
        view.begin_archive_confirmation(selected);
        return;
    }
    archive_workstream(root, remote, view, &selected);
}

fn rename_selected(view: &mut NavigatorView) {
    let Some(selected) = view.selected().cloned() else {
        view.set_message("no active Workstream is selected");
        return;
    };
    if selected.provider == ProviderKind::OpenCode {
        view.set_message("OpenCode provider rename is unavailable");
        return;
    }
    if selected.host.is_remote() && !selected.host.is_reachable() {
        view.set_message("remote host is unavailable; cached state is not actionable");
        return;
    }
    view.begin_rename(selected);
}

fn rename_workstream(
    root: &StateRoot,
    remote: &mut RemoteMonitor,
    view: &mut NavigatorView,
    selected: &NavigatorWorkstream,
    name: &str,
) {
    match run_rename_action(root, selected, name) {
        Ok(()) => {
            remote.request_soon(selected.host.alias());
            refresh_view(root, remote, view);
            view.set_message(format!(
                "canonical {} thread title updated",
                provider_label(selected.provider)
            ));
        }
        Err(error) => view.set_message(action_message(&error)),
    }
}

fn archive_workstream(
    root: &StateRoot,
    remote: &mut RemoteMonitor,
    view: &mut NavigatorView,
    selected: &NavigatorWorkstream,
) {
    match run_action(
        root,
        "archive",
        selected,
        Some(selected.workstream_revision.value()),
    ) {
        Ok(()) => {
            view.clear_attached(selected);
            remote.request_soon(selected.host.alias());
            refresh_view(root, remote, view);
            view.set_message(
                "Workstream archived; provider history and Project files are retained",
            );
        }
        Err(error) => view.set_message(action_message(&error)),
    }
}

fn restore_selected(root: &StateRoot, remote: &mut RemoteMonitor, view: &mut NavigatorView) {
    let Some(selected) = view.selected().cloned() else {
        view.set_message("no archived Workstream is selected");
        return;
    };
    if !selected.archived {
        view.set_message("select Archived to restore this Workstream");
        return;
    }
    match run_action(
        root,
        "restore",
        &selected,
        Some(selected.workstream_revision.value()),
    ) {
        Ok(()) => {
            remote.request_soon(selected.host.alias());
            refresh_view(root, remote, view);
            view.set_message(format!(
                "Workstream restored; select it to start or resume {}",
                provider_label(selected.provider)
            ));
        }
        Err(error) => view.set_message(action_message(&error)),
    }
}

fn recover_selected_operation(
    root: &StateRoot,
    presentation: &Presentation,
    remote: &mut RemoteMonitor,
    view: &mut NavigatorView,
) {
    let Some(operation) = view.selected_fork_recovery_operation().cloned() else {
        view.set_message("no unfinished Fork is selected");
        return;
    };
    recover_operation(root, presentation, remote, view, &operation);
}

fn recover_operation(
    root: &StateRoot,
    presentation: &Presentation,
    remote: &mut RemoteMonitor,
    view: &mut NavigatorView,
    operation: &NavigatorOperation,
) {
    match run_recovery_operation(root, operation) {
        Ok(destination) => {
            remote.request_soon(operation.host.alias());
            refresh_view(root, remote, view);
            view.dismiss_detail();
            if view.select_workstream(operation.host.alias(), destination) {
                activate_selected(
                    root,
                    presentation,
                    remote,
                    view,
                    WorkstreamActivationInput::ProviderHandoff,
                );
                return;
            }
            let attachment = if operation.host.is_remote() {
                presentation.attach_remote_workstream(operation.host.alias(), destination)
            } else {
                presentation.attach_workstream(destination)
            };
            match attachment.and_then(|status| {
                view.observe_attachment(&status);
                presentation.focus_provider()
            }) {
                Ok(()) => view.set_message("earlier Fork reconciled and opened"),
                Err(error) => view.set_message(action_message(&error)),
            }
        }
        Err(error) => view.set_message(action_message(&error)),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::navigator) enum CreationAction {
    Independent,
    Fork,
}

impl CreationAction {
    const fn local_command(self) -> &'static str {
        match self {
            Self::Independent => "new-workstream",
            Self::Fork => "fork-workstream",
        }
    }

    const fn remote_command(self) -> &'static str {
        match self {
            Self::Independent => "new",
            Self::Fork => "fork",
        }
    }

    pub(in crate::navigator) fn success_message(self, provider: Option<ProviderKind>) -> String {
        match self {
            Self::Independent => provider.map_or_else(
                || "new Workstream started; use the native provider UI directly".to_owned(),
                |provider| {
                    let provider = provider_label(provider);
                    format!(
                        "new {provider} Workstream started; use the native {provider} UI directly"
                    )
                },
            ),
            Self::Fork => "forked Workstream started at the last completed native turn".to_owned(),
        }
    }
}

fn create_workstream_selected(
    root: &StateRoot,
    presentation: &Presentation,
    remote: &mut RemoteMonitor,
    view: &mut NavigatorView,
    action: CreationAction,
) {
    let Some(source) = view.selected().cloned() else {
        if view.snapshot.workstreams.is_empty() {
            view.begin_checkout_registration();
        } else {
            view.set_message("select a ProjectLocation to start a new Workstream");
        }
        return;
    };
    if action == CreationAction::Fork {
        create_workstream_from_source(
            root,
            presentation,
            remote,
            view,
            &source,
            action,
            true,
            None,
        );
        return;
    }
    match view.provider_choice_for_new(&source) {
        ProviderChoice::None => {
            view.set_message("no provider is currently eligible for a new Workstream");
        }
        ProviderChoice::Immediate(provider) => create_workstream_from_source(
            root,
            presentation,
            remote,
            view,
            &source,
            action,
            true,
            Some(provider),
        ),
        ProviderChoice::Modal {
            providers,
            selected,
        } => {
            view.modal = Some(NavigatorModal::SelectProvider {
                providers,
                selected,
                intent: ProviderChoiceIntent::New { source },
            });
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn create_workstream_from_source(
    root: &StateRoot,
    presentation: &Presentation,
    remote: &mut RemoteMonitor,
    view: &mut NavigatorView,
    source: &NavigatorWorkstream,
    action: CreationAction,
    check_existing_fork: bool,
    provider: Option<ProviderKind>,
) {
    if action == CreationAction::Fork && check_existing_fork && view.begin_fork_recovery(source) {
        return;
    }
    let destination = match run_creation_action(root, action, source, provider) {
        Ok(workstream_id) => workstream_id,
        Err(error) => {
            if action == CreationAction::Fork {
                remote.request_soon(source.host.alias());
                refresh_view(root, remote, view);
                if view.begin_fork_recovery(source) {
                    return;
                }
            }
            view.set_message(action_message(&error));
            return;
        }
    };
    remote.request_soon(source.host.alias());
    refresh_view(root, remote, view);
    if view.select_workstream(source.host.alias(), destination) {
        activate_selected(
            root,
            presentation,
            remote,
            view,
            WorkstreamActivationInput::ProviderHandoff,
        );
        return;
    }
    // A remote poll is asynchronous. Its control action has already created
    // and started the exact target, so attach it directly instead of making
    // the user repeat an action while waiting for the next bounded snapshot.
    let attachment = if source.host.is_remote() {
        presentation.attach_remote_workstream(source.host.alias(), destination)
    } else {
        presentation.attach_workstream(destination)
    };
    match attachment.and_then(|status| {
        view.observe_attachment(&status);
        presentation.focus_provider()
    }) {
        Ok(()) => view.set_message(action.success_message(provider)),
        Err(error) => view.set_message(action_message(&error)),
    }
}

fn open_project_browser(
    root: &StateRoot,
    view: &mut NavigatorView,
    host: NavigatorHost,
    relative_path: &str,
) {
    if host.is_remote() && !host.is_reachable() {
        view.set_message("remote host is unavailable; Project browser was not opened");
        return;
    }
    match project_directories(root, &host, relative_path) {
        Ok(directories) => {
            view.modal = Some(NavigatorModal::ProjectBrowser {
                host,
                directories,
                selected: 0,
                scroll: 0,
                filter: String::new(),
            });
            view.normalize_project_browser_selection();
        }
        Err(error) => {
            view.set_message(action_message(&error));
        }
    }
}

fn project_directories(
    root: &StateRoot,
    host: &NavigatorHost,
    relative_path: &str,
) -> Result<ProjectDirectoriesResponse, NavigatorError> {
    let client = HostClient::new(SystemCommandRunner);
    if host.is_remote() {
        let endpoint = checked_navigator_ssh_endpoint(root, host.alias())?;
        return Ok(client.project_directories_ssh(&endpoint, relative_path)?);
    }
    let executable = std::env::current_exe().map_err(NavigatorError::CurrentExecutable)?;
    let endpoint = LocalEndpoint {
        executable,
        state_root: root.base().to_path_buf(),
    };
    Ok(client.project_directories_local(&endpoint, relative_path)?)
}

fn checked_navigator_ssh_endpoint(
    root: &StateRoot,
    alias: &str,
) -> Result<SshEndpoint, NavigatorError> {
    let catalog = ClientCatalog::open(root)?;
    let host = catalog.host(alias)?.ok_or(StateError::UnknownClientHost)?;
    let ClientHostTransport::Ssh { ref destination } = host.transport else {
        return Err(StateError::ClientHostRegistrationMismatch.into());
    };
    let destination = SshDestination::parse(destination)?;
    let executable = host
        .executable_path
        .to_str()
        .ok_or(StateError::InvalidClientHostField("remote executable"))
        .and_then(|value| {
            RemoteExecutable::parse(value)
                .map_err(|_| StateError::InvalidClientHostField("remote executable"))
        })?;
    let endpoint = SshEndpoint::new(destination, executable);
    let client = HostClient::new(SystemCommandRunner);
    client
        .probe_ssh(&endpoint)?
        .ensure_compatible_with_local()?;
    let hello = client.hello_ssh(&endpoint, "wsnav")?;
    host.verify_hello(&hello)?;
    Ok(endpoint)
}

pub(in crate::navigator) fn register_project_browser_directory(
    root: &StateRoot,
    remote: &mut RemoteMonitor,
    view: &mut NavigatorView,
    host: &NavigatorHost,
    relative_path: &str,
) {
    if host.is_remote() && !host.is_reachable() {
        view.set_message("remote host is unavailable; Project registration was not sent");
        return;
    }
    let providers = view.eligible_providers_for_host(host);
    match providers.as_slice() {
        [] => {
            view.set_message("no provider is currently eligible for a new Workstream");
        }
        [provider] => register_project_browser_directory_with_provider(
            root,
            remote,
            view,
            host,
            relative_path,
            *provider,
        ),
        _ => {
            view.modal = Some(NavigatorModal::SelectProvider {
                providers,
                selected: 0,
                intent: ProviderChoiceIntent::Register {
                    host: host.clone(),
                    relative_path: relative_path.to_owned(),
                },
            });
        }
    }
}

fn register_project_browser_directory_with_provider(
    root: &StateRoot,
    remote: &mut RemoteMonitor,
    view: &mut NavigatorView,
    host: &NavigatorHost,
    relative_path: &str,
    provider: ProviderKind,
) {
    let action = crate::protocol::HostAction::RegisterProjectDirectory {
        relative_path: relative_path.to_owned(),
        provider,
    };
    let client = HostClient::new(SystemCommandRunner);
    let result = if host.is_remote() {
        checked_navigator_ssh_endpoint(root, host.alias())
            .and_then(|endpoint| client.create_ssh(&endpoint, action).map_err(Into::into))
    } else {
        let executable = match std::env::current_exe() {
            Ok(executable) => executable,
            Err(error) => {
                view.set_message(action_message(&error));
                return;
            }
        };
        let endpoint = LocalEndpoint {
            executable,
            state_root: root.base().to_path_buf(),
        };
        client.create_local(&endpoint, action).map_err(Into::into)
    };
    match result {
        Ok(workstream_id) => {
            remote.request_soon(host.alias());
            refresh_view(root, remote, view);
            if view.select_project_for_workstream(host.alias(), workstream_id) {
                view.set_message(format!(
                    "Project registered for {}; select it to start {}",
                    provider_label(provider),
                    provider_label(provider)
                ));
            } else if host.is_remote() {
                view.set_message(format!(
                    "remote {} Project registered; waiting for its bounded snapshot",
                    provider_label(provider)
                ));
            } else {
                view.set_message(format!(
                    "{} Project registration completed; refresh the Project view",
                    provider_label(provider)
                ));
            }
        }
        Err(error) => view.set_message(action_message(&error)),
    }
}

fn configure_project_browser_root(
    root: &StateRoot,
    view: &mut NavigatorView,
    host: &NavigatorHost,
    root_path: &str,
) {
    if host.is_remote() && !host.is_reachable() {
        view.set_message("remote host is unavailable; project browser root was not changed");
        return;
    }
    let action = crate::protocol::HostAction::SetProjectBrowserRoot {
        root_path: root_path.trim().to_owned(),
    };
    let client = HostClient::new(SystemCommandRunner);
    let result = if host.is_remote() {
        checked_navigator_ssh_endpoint(root, host.alias()).and_then(|endpoint| {
            client
                .apply_ssh(&endpoint, action)
                .map(|_| ())
                .map_err(Into::into)
        })
    } else {
        let executable = match std::env::current_exe() {
            Ok(executable) => executable,
            Err(error) => {
                view.set_message(action_message(&error));
                return;
            }
        };
        let endpoint = LocalEndpoint {
            executable,
            state_root: root.base().to_path_buf(),
        };
        client
            .apply_local(&endpoint, action)
            .map(|_| ())
            .map_err(Into::into)
    };
    match result {
        Ok(()) => view.set_message("project browser root updated; use Projects → a to browse"),
        Err(error) => view.set_message(action_message(&error)),
    }
}

fn run_navigator_command(root: &StateRoot, arguments: &[&str]) -> Result<(), NavigatorError> {
    let executable = std::env::current_exe().map_err(NavigatorError::ActionLaunch)?;
    let mut command = Command::new(executable);
    command.arg("--state-root").arg(root.base()).args(arguments);
    let output =
        output_bounded(&mut command, 1024, 1024).map_err(NavigatorError::from_action_process)?;
    if output.status.success() {
        Ok(())
    } else {
        Err(NavigatorError::ActionFailed)
    }
}

fn register_remote_host(
    root: &StateRoot,
    presentation: &Presentation,
    remote: &mut RemoteMonitor,
    view: &mut NavigatorView,
    destination: &str,
) {
    let destination = destination.trim();
    match run_navigator_command(root, &["register-remote", destination]) {
        Ok(()) => {
            remote.request_soon(destination);
            refresh_view(root, remote, view);
            view.selected_host = Some(destination.to_owned());
            activate_selected_host(root, presentation, remote, view);
        }
        Err(error) => view.set_message(action_message(&error)),
    }
}

fn activate_selected_host(
    root: &StateRoot,
    presentation: &Presentation,
    remote: &mut RemoteMonitor,
    view: &mut NavigatorView,
) {
    let Some(host) = view.selected_host_summary() else {
        view.set_message("no Host is selected");
        return;
    };
    if host.observer_status == ObserverStatus::Ready {
        view.set_message("observer is already ready on this host");
        return;
    }
    if let RemoteHostReachability::Unreachable(issue) = host.reachability {
        view.set_message(format!(
            "{} is {}; observer activation was not sent",
            host.alias,
            issue.label()
        ));
        return;
    }
    let prepared = if host.alias == "local" {
        run_navigator_command(root, &["setup", "--skip-review"])
    } else {
        run_navigator_command(root, &["host", "prepare-observer", &host.alias])
    };
    if let Err(error) = prepared {
        view.set_message(action_message(&error));
        return;
    }
    let review = if host.alias == "local" {
        presentation.start_observer_review()
    } else {
        presentation.start_remote_observer_review(&host.alias)
    };
    match review.and_then(|()| presentation.focus_provider()) {
        Ok(()) => {
            remote.request_soon(&host.alias);
            refresh_view(root, remote, view);
            view.set_message(
                "approve the exact observer hooks in the native Codex pane, then exit Codex",
            );
        }
        Err(error) => view.set_message(action_message(&error)),
    }
}

pub(in crate::navigator) fn forget_selected_host(view: &mut NavigatorView) {
    let Some(host) = view.selected_host_summary() else {
        view.set_message("no Host is selected");
        return;
    };
    if host.alias == "local" {
        view.set_message("the local Host is protected and cannot be forgotten");
        return;
    }
    view.begin_host_forget(host);
}

fn forget_host(
    root: &StateRoot,
    remote: &mut RemoteMonitor,
    view: &mut NavigatorView,
    alias: &str,
    workstream_count: usize,
    location_count: usize,
    unresolved_operation_count: usize,
) {
    // `host reset` changes only this client catalog. It deliberately does not
    // call the remote host or alter retained remote workstreams.
    match run_navigator_command(root, &["host", "reset", alias]) {
        Ok(()) => {
            refresh_view(root, remote, view);
            view.set_message(format!(
                "forgot {alias}: {workstream_count} remote Workstreams and {unresolved_operation_count} operation{} retained; {location_count} local Project location{} removed",
                if unresolved_operation_count == 1 { "" } else { "s" },
                if location_count == 1 { "" } else { "s" },
            ));
        }
        Err(error) => view.set_message(action_message(&error)),
    }
}

pub(in crate::navigator) fn offboard_host(
    root: &StateRoot,
    remote: &mut RemoteMonitor,
    view: &mut NavigatorView,
    alias: &str,
    workstream_count: usize,
    location_count: usize,
    unresolved_operation_count: usize,
) {
    if view.live_runtime_count(alias) > 0 {
        view.set_message("park live Workstreams before offboarding this host");
        return;
    }
    let Some(host) = view.hosts().into_iter().find(|host| host.alias == alias) else {
        view.set_message("the selected Host is unavailable; refresh the navigator");
        return;
    };
    if let RemoteHostReachability::Unreachable(issue) = host.reachability {
        view.set_message(format!(
            "{} is {}; offboarding was not sent",
            host.alias,
            issue.label()
        ));
        return;
    }
    if host.observer_status != ObserverStatus::NotInstalled
        && let Err(error) = run_navigator_command(root, &["host", "remove-observer", alias])
    {
        view.set_message(action_message(&error));
        return;
    }
    forget_host(
        root,
        remote,
        view,
        alias,
        workstream_count,
        location_count,
        unresolved_operation_count,
    );
}

fn forget_project(
    root: &StateRoot,
    remote: &mut RemoteMonitor,
    view: &mut NavigatorView,
    project_id: ProjectId,
    label: &str,
    archived_workstream_count: usize,
    location_count: usize,
) {
    let still_active = view
        .snapshot
        .workstreams
        .iter()
        .any(|workstream| workstream.project_id == project_id && !workstream.archived);
    if still_active {
        view.set_message("the Project changed; archive its active Workstreams before removing it");
        return;
    }
    let result = ClientCatalog::open(root)
        .and_then(|mut catalog| catalog.ignore_project_locations(project_id));
    match result {
        Ok(_) => {
            remote.remove_project(project_id);
            view.snapshot
                .workstreams
                .retain(|workstream| workstream.project_id != project_id);
            view.normalize_page_selection();
            view.normalize_workstream_selection();
            view.set_message(format!(
                "removed {label}: {archived_workstream_count} archived Workstream{} at {location_count} location{} remain on their hosts",
                if archived_workstream_count == 1 { "" } else { "s" },
                if location_count == 1 { "" } else { "s" },
            ));
        }
        Err(error) => view.set_message(action_message(&error)),
    }
}

fn refresh_attachment_status(presentation: &Presentation, view: &mut NavigatorView) -> bool {
    match presentation.attachment_status() {
        Ok(Some(status)) => view.observe_attachment(&status),
        Ok(None) => false,
        Err(error) => {
            view.set_message(action_message(&error));
            true
        }
    }
}

fn refresh_view(root: &StateRoot, remote: &mut RemoteMonitor, view: &mut NavigatorView) {
    let selected_host = view.selected_host_alias().map(str::to_owned);
    match combined_snapshot(root, remote, selected_host.as_deref()) {
        Ok(snapshot) => {
            view.replace_snapshot(snapshot);
        }
        Err(error) => view.set_message(action_message(&error)),
    }
}

fn run_action(
    root: &StateRoot,
    action: &str,
    workstream: &NavigatorWorkstream,
    revision: Option<i64>,
) -> Result<(), NavigatorError> {
    let executable = std::env::current_exe().map_err(NavigatorError::ActionLaunch)?;
    let mut command = Command::new(executable);
    command.arg("--state-root").arg(root.base());
    if workstream.host.is_remote() {
        if !workstream.host.is_reachable() {
            return Err(NavigatorError::RemoteHostUnavailable);
        }
        command
            .arg("host")
            .arg(action)
            .arg(workstream.host.alias())
            .arg(workstream.workstream_id.to_string());
        let revision = revision.unwrap_or_else(|| workstream.workstream_revision.value());
        command.arg(revision.to_string());
    } else {
        command
            .arg(action)
            .arg(workstream.workstream_id.to_string());
        if let Some(revision) = revision {
            command.arg(revision.to_string());
        }
    }
    let output =
        output_bounded(&mut command, 1024, 1024).map_err(NavigatorError::from_action_process)?;
    if output.status.success() {
        Ok(())
    } else {
        Err(NavigatorError::ActionFailed)
    }
}

fn run_creation_action(
    root: &StateRoot,
    action: CreationAction,
    source: &NavigatorWorkstream,
    provider: Option<ProviderKind>,
) -> Result<WorkstreamId, NavigatorError> {
    if source.host.is_remote() && !source.host.is_reachable() {
        return Err(NavigatorError::RemoteHostUnavailable);
    }
    let executable = std::env::current_exe().map_err(NavigatorError::ActionLaunch)?;
    let mut command = Command::new(executable);
    command.arg("--state-root").arg(root.base());
    command.args(creation_command_arguments(action, source, provider)?);
    let output =
        output_bounded(&mut command, 1024, 1024).map_err(NavigatorError::from_action_process)?;
    if !output.status.success() {
        return Err(NavigatorError::ActionFailed);
    }
    parse_created_workstream(&output.stdout)
}

pub(in crate::navigator) fn creation_command_arguments(
    action: CreationAction,
    source: &NavigatorWorkstream,
    provider: Option<ProviderKind>,
) -> Result<Vec<String>, NavigatorError> {
    let mut arguments = if source.host.is_remote() {
        vec![
            "host".to_owned(),
            action.remote_command().to_owned(),
            source.host.alias().to_owned(),
            source.workstream_id.to_string(),
            source.workstream_revision.value().to_string(),
        ]
    } else {
        vec![
            action.local_command().to_owned(),
            source.workstream_id.to_string(),
        ]
    };
    if action == CreationAction::Independent {
        arguments.push("--provider".to_owned());
        arguments.push(
            provider
                .ok_or_else(|| {
                    NavigatorError::ProviderSelection(
                        crate::provider::ProviderSelectionError::SelectionRequired,
                    )
                })?
                .as_str()
                .to_owned(),
        );
    }
    Ok(arguments)
}

fn run_recovery_operation(
    root: &StateRoot,
    operation: &NavigatorOperation,
) -> Result<WorkstreamId, NavigatorError> {
    if operation.host.is_remote() {
        if !operation.host.is_reachable() {
            return Err(NavigatorError::RemoteHostUnavailable);
        }
        let endpoint = checked_navigator_ssh_endpoint(root, operation.host.alias())?;
        return HostClient::new(SystemCommandRunner)
            .create_ssh(
                &endpoint,
                HostAction::RecoverOperation {
                    operation_id: operation.operation_id,
                },
            )
            .map_err(Into::into);
    }
    let executable = std::env::current_exe().map_err(NavigatorError::CurrentExecutable)?;
    let endpoint = LocalEndpoint {
        executable,
        state_root: root.base().to_path_buf(),
    };
    HostClient::new(SystemCommandRunner)
        .create_local(
            &endpoint,
            HostAction::RecoverOperation {
                operation_id: operation.operation_id,
            },
        )
        .map_err(Into::into)
}

fn run_rename_action(
    root: &StateRoot,
    workstream: &NavigatorWorkstream,
    name: &str,
) -> Result<(), NavigatorError> {
    if workstream.host.is_remote() && !workstream.host.is_reachable() {
        return Err(NavigatorError::RemoteHostUnavailable);
    }
    let executable = std::env::current_exe().map_err(NavigatorError::ActionLaunch)?;
    let mut command = Command::new(executable);
    command.arg("--state-root").arg(root.base());
    if workstream.host.is_remote() {
        command
            .arg("host")
            .arg("rename")
            .arg(workstream.host.alias())
            .arg(workstream.workstream_id.to_string())
            .arg(workstream.workstream_revision.value().to_string())
            .arg(name);
    } else {
        command
            .arg("rename")
            .arg(workstream.workstream_id.to_string())
            .arg(workstream.workstream_revision.value().to_string())
            .arg(name);
    }
    let output =
        output_bounded(&mut command, 1024, 1024).map_err(NavigatorError::from_action_process)?;
    if output.status.success() {
        Ok(())
    } else {
        Err(NavigatorError::ActionFailed)
    }
}

pub(in crate::navigator) fn parse_created_workstream(
    output: &[u8],
) -> Result<WorkstreamId, NavigatorError> {
    let output = std::str::from_utf8(output).map_err(|_| NavigatorError::InvalidActionResult)?;
    let Some(identifier) = output.split_whitespace().last() else {
        return Err(NavigatorError::InvalidActionResult);
    };
    WorkstreamId::from_str(identifier).map_err(|_| NavigatorError::InvalidActionResult)
}

fn action_message(error: &impl std::fmt::Display) -> String {
    bounded_display(&error.to_string())
}

struct TerminalSession {
    terminal: Terminal<CrosstermBackend<io::Stdout>>,
}

impl TerminalSession {
    fn enter() -> Result<Self, io::Error> {
        enable_raw_mode()?;
        let mut output = stdout();
        if let Err(error) = execute!(output, EnterAlternateScreen, EnableMouseCapture) {
            let _ = disable_raw_mode();
            return Err(error);
        }
        match Terminal::new(CrosstermBackend::new(output)) {
            Ok(terminal) => Ok(Self { terminal }),
            Err(error) => {
                let _ = disable_raw_mode();
                let _ = execute!(stdout(), LeaveAlternateScreen, DisableMouseCapture);
                Err(error)
            }
        }
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(
            self.terminal.backend_mut(),
            LeaveAlternateScreen,
            DisableMouseCapture
        );
        let _ = self.terminal.show_cursor();
    }
}
