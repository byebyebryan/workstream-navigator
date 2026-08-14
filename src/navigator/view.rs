use std::{collections::BTreeSet, time::Instant};

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListState, Paragraph, Wrap},
};

use crate::{
    domain::{LocationId, OperationKind, ProjectId, ProviderKind, WorkstreamId},
    presentation::{AttachmentPhase, AttachmentStatus},
    protocol::{ObserverStatus, ProjectDirectoriesResponse, ProjectDirectoryEntry},
};

use super::model::{
    LocalNavigatorSnapshot, NavigatorHost, NavigatorOperation, NavigatorRuntimeStatus,
    NavigatorWorkstream, RemoteHostIssue, RemoteHostReachability, operation_kind_label,
    operation_phase_label, provider_label,
};
use super::render::{
    ATTACHMENT_READY_MESSAGE_DURATION, COMPACT_HINT_LEFT_INSET, STATUS_BOX_HEIGHT, binding_line,
    help_lines, host_overview_height, host_overview_item, navigator_list_item,
    navigator_modal_area, navigator_modal_content, project_browser_entry_indexes,
    project_browser_scroll_to_selected, project_overview_height, project_overview_item,
    selected_row_style, visible_project_colors,
};
use super::snapshot::bounded_display;

/// Pure navigator selection and rendering state.
#[derive(Clone, Debug, Default)]
pub struct NavigatorView {
    pub(in crate::navigator) snapshot: LocalNavigatorSnapshot,
    pub(in crate::navigator) selected: usize,
    pub(in crate::navigator) page: NavigatorPage,
    pub(in crate::navigator) detail: Option<NavigatorDetail>,
    pub(in crate::navigator) selected_project: Option<ProjectId>,
    pub(in crate::navigator) selected_host: Option<String>,
    pub(in crate::navigator) selected_operation: usize,
    pub(in crate::navigator) view_mode: NavigatorViewMode,
    pub(in crate::navigator) attached: Option<(String, WorkstreamId)>,
    pub(in crate::navigator) observed_attachment: Option<(uuid::Uuid, AttachmentPhase)>,
    pub(in crate::navigator) rendered_offset: usize,
    pub(in crate::navigator) rendered_mouse_rows: Vec<(u16, usize)>,
    pub(in crate::navigator) rendered_project_rows: Vec<(u16, ProjectId)>,
    pub(in crate::navigator) rendered_host_rows: Vec<(u16, String)>,
    pub(in crate::navigator) mouse_click: Option<MouseClickIntent>,
    pub(in crate::navigator) message: Option<String>,
    pub(in crate::navigator) transient_message: Option<(String, Instant)>,
    pub(in crate::navigator) help_visible: bool,
    pub(in crate::navigator) help_scroll: u16,
    pub(in crate::navigator) modal: Option<NavigatorModal>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::navigator) enum MouseClickIntent {
    Blank,
    Row,
    Management,
}

/// The active navigator page. This remains process-local presentation state.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(in crate::navigator) enum NavigatorPage {
    #[default]
    Workstreams,
    Projects,
    Hosts,
}

impl NavigatorPage {
    pub(in crate::navigator) const fn label(self) -> &'static str {
        match self {
            Self::Workstreams => "Workstreams",
            Self::Projects => "Projects",
            Self::Hosts => "Hosts",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::navigator) enum NavigatorDetail {
    ForkRecovery {
        host_alias: String,
        source_workstream_id: WorkstreamId,
    },
    Workstream {
        host_alias: String,
        workstream_id: WorkstreamId,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::navigator) enum NavigatorModal {
    ConfirmArchive(NavigatorWorkstream),
    ConfirmForkRecovery {
        source: NavigatorWorkstream,
        operation: NavigatorOperation,
    },
    SelectHostRemoval {
        alias: String,
        workstream_count: usize,
        location_count: usize,
        unresolved_operation_count: usize,
        offboard: bool,
    },
    ConfirmForgetProject {
        project_id: ProjectId,
        label: String,
        archived_workstream_count: usize,
        location_count: usize,
    },
    Rename {
        workstream: NavigatorWorkstream,
        value: String,
    },
    SelectRegistrationHost {
        hosts: Vec<NavigatorHost>,
        selected: usize,
    },
    SelectProvider {
        providers: Vec<ProviderKind>,
        selected: usize,
        intent: ProviderChoiceIntent,
    },
    ProjectBrowser {
        host: NavigatorHost,
        directories: ProjectDirectoriesResponse,
        selected: usize,
        scroll: usize,
        filter: String,
        include_hidden: bool,
    },
    ConfigureProjectBrowserRoot {
        host: NavigatorHost,
        value: String,
    },
    RegisterHost {
        value: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::navigator) enum ProviderChoiceIntent {
    New {
        source: NavigatorWorkstream,
    },
    Register {
        host: NavigatorHost,
        relative_path: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::navigator) enum ProviderChoice {
    None,
    Immediate(ProviderKind),
    Modal {
        providers: Vec<ProviderKind>,
        selected: usize,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::navigator) struct NavigatorProjectOverview {
    pub(in crate::navigator) project_id: ProjectId,
    pub(in crate::navigator) label: String,
    pub(in crate::navigator) remote_identity_display: Option<String>,
    pub(in crate::navigator) workstream_count: usize,
    pub(in crate::navigator) active_workstream_count: usize,
    pub(in crate::navigator) archived_workstream_count: usize,
    pub(in crate::navigator) locations: Vec<NavigatorProjectLocation>,
    pub(in crate::navigator) latest_activity_at_millis: Option<i64>,
}

/// One host-owned `ProjectLocation` summarized without a repository path or
/// durable provider identifier. The opaque location ID remains presentation
/// action identity only.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::navigator) struct NavigatorProjectLocation {
    pub(in crate::navigator) host: NavigatorHost,
    pub(in crate::navigator) location_id: LocationId,
    pub(in crate::navigator) label: String,
    pub(in crate::navigator) active_workstream_count: usize,
    pub(in crate::navigator) archived_workstream_count: usize,
    pub(in crate::navigator) latest_activity_at_millis: Option<i64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::navigator) struct NavigatorHostSummary {
    pub(in crate::navigator) alias: String,
    pub(in crate::navigator) reachability: RemoteHostReachability,
    pub(in crate::navigator) observer_status: ObserverStatus,
    pub(in crate::navigator) workstream_count: usize,
    pub(in crate::navigator) location_count: usize,
    pub(in crate::navigator) unresolved_operation_count: usize,
    pub(in crate::navigator) latest_activity_at_millis: Option<i64>,
    pub(in crate::navigator) active_projects: Vec<NavigatorHostProject>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::navigator) struct NavigatorHostProject {
    pub(in crate::navigator) project_id: ProjectId,
    pub(in crate::navigator) label: String,
    pub(in crate::navigator) active_workstream_count: usize,
}

/// Local presentation grouping only. It is deliberately not durable state.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(in crate::navigator) enum NavigatorViewMode {
    #[default]
    Recent,
    Host,
    Project,
    Archived,
}

impl NavigatorViewMode {
    pub(in crate::navigator) const fn next(self) -> Self {
        match self {
            Self::Recent => Self::Project,
            Self::Project => Self::Host,
            Self::Host => Self::Archived,
            Self::Archived => Self::Recent,
        }
    }

    pub(in crate::navigator) const fn previous(self) -> Self {
        match self {
            Self::Recent => Self::Archived,
            Self::Project => Self::Recent,
            Self::Host => Self::Project,
            Self::Archived => Self::Host,
        }
    }

    pub(in crate::navigator) const fn label(self) -> &'static str {
        match self {
            Self::Recent => "Recent",
            Self::Host => "By host",
            Self::Project => "By project",
            Self::Archived => "Archived",
        }
    }

    pub(in crate::navigator) const fn includes(self, workstream: &NavigatorWorkstream) -> bool {
        match self {
            Self::Archived => workstream.archived,
            Self::Recent | Self::Host | Self::Project => !workstream.archived,
        }
    }

    pub(in crate::navigator) const fn is_archived(self) -> bool {
        matches!(self, Self::Archived)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::navigator) enum WorkstreamRowContext {
    Recent,
    Archived,
    Host,
    Project,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::navigator) enum NavigatorListEntry {
    HostHeader {
        alias: String,
    },
    ProjectHeader {
        project_id: ProjectId,
        label: String,
    },
    Workstream {
        snapshot_index: usize,
        context: WorkstreamRowContext,
        tree_branch: Option<TreeBranch>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::navigator) struct TreeBranch {
    pub(in crate::navigator) is_last: bool,
}

impl NavigatorListEntry {
    pub(in crate::navigator) const fn height(&self) -> u16 {
        match self {
            Self::HostHeader { .. } | Self::ProjectHeader { .. } => 1,
            Self::Workstream {
                context: WorkstreamRowContext::Recent,
                ..
            } => 3,
            Self::Workstream { .. } => 2,
        }
    }

    pub(in crate::navigator) const fn workstream_index(&self) -> Option<usize> {
        match self {
            Self::Workstream { snapshot_index, .. } => Some(*snapshot_index),
            Self::HostHeader { .. } | Self::ProjectHeader { .. } => None,
        }
    }
}

impl NavigatorView {
    #[must_use]
    pub fn new(snapshot: LocalNavigatorSnapshot) -> Self {
        let mut view = Self {
            snapshot,
            selected: 0,
            page: NavigatorPage::Workstreams,
            detail: None,
            selected_project: None,
            selected_host: Some("local".to_owned()),
            selected_operation: 0,
            view_mode: NavigatorViewMode::Recent,
            attached: None,
            observed_attachment: None,
            rendered_offset: 0,
            rendered_mouse_rows: Vec::new(),
            rendered_project_rows: Vec::new(),
            rendered_host_rows: Vec::new(),
            mouse_click: None,
            message: None,
            transient_message: None,
            help_visible: false,
            help_scroll: 0,
            modal: None,
        };
        view.normalize_page_selection();
        view
    }

    pub fn replace_snapshot(&mut self, snapshot: LocalNavigatorSnapshot) -> bool {
        let snapshot_changed = self.snapshot != snapshot;
        let previous_selected = self.selected;
        let previous_attachment = self.attached.clone();
        let selected_id = self
            .selected()
            .map(|row| (row.host.alias().to_owned(), row.workstream_id));
        self.snapshot = snapshot;
        self.selected = selected_id
            .and_then(|(host_alias, workstream_id)| {
                self.snapshot.workstreams.iter().position(|row| {
                    row.host.alias() == host_alias && row.workstream_id == workstream_id
                })
            })
            .unwrap_or_else(|| {
                self.selected
                    .min(self.snapshot.workstreams.len().saturating_sub(1))
            });
        self.clear_inactive_attachment();
        self.normalize_page_selection();
        self.normalize_workstream_selection();
        snapshot_changed
            || self.selected != previous_selected
            || self.attached != previous_attachment
    }

    #[must_use]
    pub fn selected(&self) -> Option<&NavigatorWorkstream> {
        self.workstream_is_visible(self.selected)
            .then(|| self.snapshot.workstreams.get(self.selected))
            .flatten()
    }

    pub(in crate::navigator) fn workstream_is_visible(&self, snapshot_index: usize) -> bool {
        self.snapshot
            .workstreams
            .get(snapshot_index)
            .is_some_and(|workstream| self.view_mode.includes(workstream))
    }

    pub(in crate::navigator) fn visible_workstream_indexes(&self) -> Vec<usize> {
        self.list_entries()
            .into_iter()
            .filter_map(|entry| entry.workstream_index())
            .collect()
    }

    pub(in crate::navigator) fn normalize_workstream_selection(&mut self) {
        if !self.workstream_is_visible(self.selected) {
            self.selected = self
                .visible_workstream_indexes()
                .into_iter()
                .next()
                .unwrap_or(0);
        }
    }

    pub(in crate::navigator) fn selected_host_alias(&self) -> Option<&str> {
        if let Some(NavigatorDetail::ForkRecovery { host_alias, .. }) = &self.detail {
            Some(host_alias)
        } else if self.page == NavigatorPage::Hosts {
            self.selected_host.as_deref()
        } else {
            self.selected().map(|row| row.host.alias())
        }
    }

    pub(in crate::navigator) const fn page(&self) -> NavigatorPage {
        self.page
    }

    pub(in crate::navigator) fn select_page(&mut self, page: NavigatorPage) {
        if self.page != page {
            self.page = page;
            self.detail = None;
            self.clear_message();
        }
    }

    pub(in crate::navigator) fn toggle_management_page(&mut self, page: NavigatorPage) {
        debug_assert_ne!(page, NavigatorPage::Workstreams);
        self.select_page(if self.page == page {
            NavigatorPage::Workstreams
        } else {
            page
        });
    }

    pub(in crate::navigator) fn open_selected_detail(&mut self) {
        if self.page == NavigatorPage::Workstreams {
            self.detail = self
                .selected()
                .map(|workstream| NavigatorDetail::Workstream {
                    host_alias: workstream.host.alias().to_owned(),
                    workstream_id: workstream.workstream_id,
                });
        }
    }

    pub(in crate::navigator) fn fork_recovery_operations(&self) -> Vec<&NavigatorOperation> {
        let Some(NavigatorDetail::ForkRecovery {
            host_alias,
            source_workstream_id,
        }) = &self.detail
        else {
            return Vec::new();
        };
        self.snapshot
            .unresolved_operations
            .iter()
            .filter(|operation| {
                operation.kind == OperationKind::Fork
                    && operation.host.alias() == host_alias
                    && operation.source_workstream_id == Some(*source_workstream_id)
            })
            .collect()
    }

    pub(in crate::navigator) fn selected_fork_recovery_operation(
        &self,
    ) -> Option<&NavigatorOperation> {
        self.fork_recovery_operations()
            .get(self.selected_operation)
            .copied()
    }

    pub(in crate::navigator) fn begin_fork_recovery(
        &mut self,
        source: &NavigatorWorkstream,
    ) -> bool {
        let operations = self
            .snapshot
            .unresolved_operations
            .iter()
            .filter(|operation| {
                operation.kind == OperationKind::Fork
                    && operation.host.alias() == source.host.alias()
                    && operation.source_workstream_id == Some(source.workstream_id)
            })
            .cloned()
            .collect::<Vec<_>>();
        match operations.as_slice() {
            [] => false,
            [operation] => {
                self.modal = Some(NavigatorModal::ConfirmForkRecovery {
                    source: source.clone(),
                    operation: operation.clone(),
                });
                true
            }
            _ => {
                self.selected_operation = 0;
                self.detail = Some(NavigatorDetail::ForkRecovery {
                    host_alias: source.host.alias().to_owned(),
                    source_workstream_id: source.workstream_id,
                });
                self.set_message("choose the exact unfinished Fork to reconcile");
                true
            }
        }
    }

    pub(in crate::navigator) fn dismiss_detail(&mut self) -> bool {
        self.detail.take().is_some()
    }

    pub(in crate::navigator) fn projects(&self) -> Vec<NavigatorProjectOverview> {
        let mut projects = Vec::<NavigatorProjectOverview>::new();
        for workstream in &self.snapshot.workstreams {
            if let Some(project) = projects
                .iter_mut()
                .find(|project| project.project_id == workstream.project_id)
            {
                if project.remote_identity_display.is_none() {
                    project
                        .remote_identity_display
                        .clone_from(&workstream.remote_identity_display);
                }
                project.workstream_count += 1;
                if workstream.archived {
                    project.archived_workstream_count += 1;
                } else {
                    project.active_workstream_count += 1;
                }
                project.latest_activity_at_millis = project
                    .latest_activity_at_millis
                    .max(workstream.last_activity_at_millis);
                if let Some(location) = project.locations.iter_mut().find(|location| {
                    location.host.alias() == workstream.host.alias()
                        && location.location_id == workstream.location_id
                }) {
                    if workstream.archived {
                        location.archived_workstream_count += 1;
                    } else {
                        location.active_workstream_count += 1;
                    }
                    location.latest_activity_at_millis = location
                        .latest_activity_at_millis
                        .max(workstream.last_activity_at_millis);
                } else {
                    project.locations.push(NavigatorProjectLocation {
                        host: workstream.host.clone(),
                        location_id: workstream.location_id,
                        label: workstream.location_label.clone(),
                        active_workstream_count: usize::from(!workstream.archived),
                        archived_workstream_count: usize::from(workstream.archived),
                        latest_activity_at_millis: workstream.last_activity_at_millis,
                    });
                }
            } else {
                projects.push(NavigatorProjectOverview {
                    project_id: workstream.project_id,
                    label: workstream.project_label.clone(),
                    remote_identity_display: workstream.remote_identity_display.clone(),
                    workstream_count: 1,
                    active_workstream_count: usize::from(!workstream.archived),
                    archived_workstream_count: usize::from(workstream.archived),
                    locations: vec![NavigatorProjectLocation {
                        host: workstream.host.clone(),
                        location_id: workstream.location_id,
                        label: workstream.location_label.clone(),
                        active_workstream_count: usize::from(!workstream.archived),
                        archived_workstream_count: usize::from(workstream.archived),
                        latest_activity_at_millis: workstream.last_activity_at_millis,
                    }],
                    latest_activity_at_millis: workstream.last_activity_at_millis,
                });
            }
        }
        for project in &mut projects {
            project.locations.sort_by(|left, right| {
                left.host
                    .alias()
                    .cmp(right.host.alias())
                    .then_with(|| left.label.cmp(&right.label))
                    .then_with(|| left.location_id.cmp(&right.location_id))
            });
        }
        projects.sort_by(|left, right| {
            right
                .latest_activity_at_millis
                .cmp(&left.latest_activity_at_millis)
                .then_with(|| left.label.cmp(&right.label))
                .then_with(|| left.project_id.cmp(&right.project_id))
        });
        projects
    }

    pub(in crate::navigator) fn select_project_for_workstream(
        &mut self,
        host_alias: &str,
        workstream_id: WorkstreamId,
    ) -> bool {
        let Some((index, project_id)) = self
            .snapshot
            .workstreams
            .iter()
            .enumerate()
            .find(|(_, workstream)| {
                workstream.host.alias() == host_alias && workstream.workstream_id == workstream_id
            })
            .map(|(index, workstream)| (index, workstream.project_id))
        else {
            return false;
        };
        self.page = NavigatorPage::Projects;
        self.detail = None;
        self.selected = index;
        self.selected_project = Some(project_id);
        true
    }

    pub(in crate::navigator) fn hosts(&self) -> Vec<NavigatorHostSummary> {
        let mut hosts = self
            .snapshot
            .hosts
            .iter()
            .map(|host| NavigatorHostSummary {
                alias: host.alias.clone(),
                reachability: host.reachability,
                observer_status: host.observer_status,
                workstream_count: 0,
                location_count: 0,
                unresolved_operation_count: 0,
                latest_activity_at_millis: None,
                active_projects: Vec::new(),
            })
            .collect::<Vec<_>>();
        let mut locations = BTreeSet::new();
        for workstream in &self.snapshot.workstreams {
            if let Some(host) = hosts
                .iter_mut()
                .find(|host| host.alias == workstream.host.alias())
            {
                host.workstream_count += 1;
                if locations.insert((workstream.host.alias().to_owned(), workstream.location_id)) {
                    host.location_count += 1;
                }
                host.latest_activity_at_millis = host
                    .latest_activity_at_millis
                    .max(workstream.last_activity_at_millis);
                if !workstream.archived {
                    if let Some(project) = host
                        .active_projects
                        .iter_mut()
                        .find(|project| project.project_id == workstream.project_id)
                    {
                        project.active_workstream_count += 1;
                    } else {
                        host.active_projects.push(NavigatorHostProject {
                            project_id: workstream.project_id,
                            label: workstream.project_label.clone(),
                            active_workstream_count: 1,
                        });
                    }
                }
            } else {
                hosts.push(NavigatorHostSummary {
                    alias: workstream.host.alias().to_owned(),
                    reachability: if workstream.host.is_reachable() {
                        RemoteHostReachability::Reachable
                    } else {
                        RemoteHostReachability::Unreachable(
                            RemoteHostIssue::ControlCommunicationFailed,
                        )
                    },
                    observer_status: ObserverStatus::NotInstalled,
                    workstream_count: 1,
                    location_count: 1,
                    unresolved_operation_count: 0,
                    latest_activity_at_millis: workstream.last_activity_at_millis,
                    active_projects: (!workstream.archived)
                        .then(|| NavigatorHostProject {
                            project_id: workstream.project_id,
                            label: workstream.project_label.clone(),
                            active_workstream_count: 1,
                        })
                        .into_iter()
                        .collect(),
                });
            }
        }
        for operation in &self.snapshot.unresolved_operations {
            if let Some(host) = hosts
                .iter_mut()
                .find(|host| host.alias == operation.host.alias())
            {
                host.unresolved_operation_count += 1;
            }
        }
        for host in &mut hosts {
            host.active_projects.sort_by(|left, right| {
                left.label
                    .cmp(&right.label)
                    .then_with(|| left.project_id.cmp(&right.project_id))
            });
        }
        hosts.sort_by(|left, right| {
            (left.alias != "local")
                .cmp(&(right.alias != "local"))
                .then_with(|| left.alias.cmp(&right.alias))
        });
        hosts
    }

    pub(in crate::navigator) fn normalize_page_selection(&mut self) {
        self.selected_operation = self
            .selected_operation
            .min(self.snapshot.unresolved_operations.len().saturating_sub(1));
        let projects = self.projects();
        if !projects
            .iter()
            .any(|project| Some(project.project_id) == self.selected_project)
        {
            self.selected_project = projects.first().map(|project| project.project_id);
        }
        let hosts = self.hosts();
        if !hosts
            .iter()
            .any(|host| Some(host.alias.as_str()) == self.selected_host.as_deref())
        {
            self.selected_host = hosts.first().map(|host| host.alias.clone());
        }
        match &self.detail {
            Some(NavigatorDetail::ForkRecovery { .. })
                if self.fork_recovery_operations().is_empty() =>
            {
                self.detail = None;
            }
            Some(NavigatorDetail::Workstream {
                host_alias,
                workstream_id,
            }) if !self.snapshot.workstreams.iter().any(|workstream| {
                workstream.host.alias() == host_alias && workstream.workstream_id == *workstream_id
            }) =>
            {
                self.detail = None;
            }
            Some(_) | None => {}
        }
    }

    pub fn select_next(&mut self) {
        if matches!(self.detail, Some(NavigatorDetail::ForkRecovery { .. })) {
            let recovery_count = self.fork_recovery_operations().len();
            if recovery_count != 0 {
                self.selected_operation = (self.selected_operation + 1) % recovery_count;
            }
            return;
        }
        if self.detail.is_some() {
            return;
        }
        match self.page {
            NavigatorPage::Workstreams => {
                let visible = self.visible_workstream_indexes();
                if let Some(index) = visible.iter().position(|index| *index == self.selected) {
                    self.selected = visible[(index + 1) % visible.len()];
                } else if let Some(first) = visible.first() {
                    self.selected = *first;
                }
            }
            NavigatorPage::Projects => {
                let projects = self.projects();
                if let Some(index) = projects
                    .iter()
                    .position(|project| Some(project.project_id) == self.selected_project)
                {
                    self.selected_project = Some(projects[(index + 1) % projects.len()].project_id);
                } else {
                    self.selected_project = projects.first().map(|project| project.project_id);
                }
            }
            NavigatorPage::Hosts => {
                let hosts = self.hosts();
                if let Some(index) = hosts
                    .iter()
                    .position(|host| Some(host.alias.as_str()) == self.selected_host.as_deref())
                {
                    self.selected_host = Some(hosts[(index + 1) % hosts.len()].alias.clone());
                } else {
                    self.selected_host = hosts.first().map(|host| host.alias.clone());
                }
            }
        }
    }

    pub fn select_previous(&mut self) {
        if matches!(self.detail, Some(NavigatorDetail::ForkRecovery { .. })) {
            let recovery_count = self.fork_recovery_operations().len();
            if recovery_count != 0 {
                self.selected_operation = self
                    .selected_operation
                    .checked_sub(1)
                    .unwrap_or(recovery_count - 1);
            }
            return;
        }
        if self.detail.is_some() {
            return;
        }
        match self.page {
            NavigatorPage::Workstreams => {
                let visible = self.visible_workstream_indexes();
                if let Some(index) = visible.iter().position(|index| *index == self.selected) {
                    self.selected = visible[(index + visible.len() - 1) % visible.len()];
                } else if let Some(first) = visible.first() {
                    self.selected = *first;
                }
            }
            NavigatorPage::Projects => {
                let projects = self.projects();
                if let Some(index) = projects
                    .iter()
                    .position(|project| Some(project.project_id) == self.selected_project)
                {
                    self.selected_project =
                        Some(projects[(index + projects.len() - 1) % projects.len()].project_id);
                } else {
                    self.selected_project = projects.first().map(|project| project.project_id);
                }
            }
            NavigatorPage::Hosts => {
                let hosts = self.hosts();
                if let Some(index) = hosts
                    .iter()
                    .position(|host| Some(host.alias.as_str()) == self.selected_host.as_deref())
                {
                    self.selected_host =
                        Some(hosts[(index + hosts.len() - 1) % hosts.len()].alias.clone());
                } else {
                    self.selected_host = hosts.first().map(|host| host.alias.clone());
                }
            }
        }
    }

    pub fn select_row(&mut self, row: usize) {
        if self.workstream_is_visible(row) {
            self.selected = row;
        }
    }

    pub(in crate::navigator) fn select_workstream(
        &mut self,
        host_alias: &str,
        workstream_id: WorkstreamId,
    ) -> bool {
        let Some(selected) = self
            .snapshot
            .workstreams
            .iter()
            .position(|row| row.host.alias() == host_alias && row.workstream_id == workstream_id)
            .filter(|index| self.workstream_is_visible(*index))
        else {
            return false;
        };
        self.selected = selected;
        true
    }

    pub(in crate::navigator) fn is_attached_to(&self, workstream: &NavigatorWorkstream) -> bool {
        self.attached
            .as_ref()
            .is_some_and(|(host_alias, workstream_id)| {
                host_alias == workstream.host.alias() && *workstream_id == workstream.workstream_id
            })
    }

    pub(in crate::navigator) fn observe_attachment(&mut self, status: &AttachmentStatus) -> bool {
        let observation = (status.attempt_id, status.phase);
        let changed = self.observed_attachment != Some(observation);
        let previous_attachment = self.attached.clone();
        self.observed_attachment = Some(observation);
        match status.phase {
            AttachmentPhase::Pending | AttachmentPhase::Running => {
                self.attached = Some((status.host_alias.clone(), status.workstream_id));
                if changed {
                    if status.phase == AttachmentPhase::Pending {
                        self.set_message("provider attachment starting");
                    } else {
                        let message = self
                            .snapshot
                            .workstreams
                            .iter()
                            .find(|workstream| {
                                workstream.host.alias() == status.host_alias
                                    && workstream.workstream_id == status.workstream_id
                            })
                            .map_or_else(
                                || {
                                    "provider attached; use the native provider UI directly"
                                        .to_owned()
                                },
                                |workstream| {
                                    let provider = provider_label(workstream.provider);
                                    format!(
                                        "{provider} attached; use the native {provider} UI directly"
                                    )
                                },
                            );
                        self.set_transient_message(message, Instant::now());
                    }
                }
            }
            AttachmentPhase::Completed => {
                self.attached = None;
                if changed {
                    self.set_message(
                        "provider detached; press Enter or click this row to reconnect",
                    );
                }
            }
            AttachmentPhase::Failed => {
                self.attached = None;
                if changed {
                    self.set_message("attachment failed; press Enter or click row to retry");
                }
            }
        }
        changed || self.attached != previous_attachment
    }

    pub(in crate::navigator) fn clear_attached(&mut self, workstream: &NavigatorWorkstream) {
        if self.is_attached_to(workstream) {
            self.attached = None;
        }
    }

    pub(in crate::navigator) fn clear_inactive_attachment(&mut self) {
        let Some((host_alias, workstream_id)) = &self.attached else {
            return;
        };
        let still_live = self.snapshot.workstreams.iter().any(|workstream| {
            workstream.host.alias() == host_alias
                && workstream.workstream_id == *workstream_id
                && !workstream.archived
                && !matches!(
                    workstream.runtime_status,
                    NavigatorRuntimeStatus::Parked | NavigatorRuntimeStatus::Unknown
                )
        });
        if !still_live {
            self.attached = None;
        }
    }

    pub(in crate::navigator) fn begin_mouse_click(&mut self, row: Option<usize>) {
        self.mouse_click = Some(if row.is_some() {
            MouseClickIntent::Row
        } else {
            MouseClickIntent::Blank
        });
        if let Some(row) = row {
            self.select_row(row);
        }
    }

    pub(in crate::navigator) fn take_mouse_click(&mut self) -> Option<MouseClickIntent> {
        self.mouse_click.take()
    }

    pub fn set_message(&mut self, message: impl Into<String>) {
        let message = bounded_display(&message.into());
        self.message = Some(message);
        self.transient_message = None;
    }

    pub fn clear_message(&mut self) {
        self.message = None;
        self.transient_message = None;
    }

    pub(in crate::navigator) fn set_transient_message(
        &mut self,
        message: impl Into<String>,
        now: Instant,
    ) {
        let message = bounded_display(&message.into());
        self.message = None;
        self.transient_message = Some((message, now + ATTACHMENT_READY_MESSAGE_DURATION));
    }

    pub(in crate::navigator) fn expire_transient_message(&mut self, now: Instant) -> bool {
        if self
            .transient_message
            .as_ref()
            .is_some_and(|(_, expires_at)| now >= *expires_at)
        {
            self.transient_message = None;
            return true;
        }
        false
    }

    pub(in crate::navigator) fn toggle_help(&mut self) {
        self.help_visible = !self.help_visible;
        self.help_scroll = 0;
    }

    pub(in crate::navigator) fn dismiss_help(&mut self) {
        self.help_visible = false;
        self.help_scroll = 0;
    }

    pub(in crate::navigator) fn begin_archive_confirmation(
        &mut self,
        workstream: NavigatorWorkstream,
    ) {
        self.modal = Some(NavigatorModal::ConfirmArchive(workstream));
    }

    pub(in crate::navigator) fn begin_rename(&mut self, workstream: NavigatorWorkstream) {
        self.modal = Some(NavigatorModal::Rename {
            value: workstream.display_name.clone(),
            workstream,
        });
    }

    pub(in crate::navigator) fn selected_host_summary(&self) -> Option<NavigatorHostSummary> {
        let alias = self.selected_host.as_deref()?;
        self.hosts().into_iter().find(|host| host.alias == alias)
    }

    pub(in crate::navigator) fn selected_host_for_project_browser(&self) -> Option<NavigatorHost> {
        let host = self.selected_host_summary()?;
        Some(if host.alias == "local" {
            NavigatorHost::Local
        } else {
            NavigatorHost::Remote {
                alias: host.alias,
                reachability: host.reachability,
            }
        })
    }

    pub(in crate::navigator) fn eligible_providers_for_host(
        &self,
        host: &NavigatorHost,
    ) -> Vec<ProviderKind> {
        self.snapshot
            .hosts
            .iter()
            .find(|overview| overview.alias == host.alias())
            .filter(|overview| overview.reachability.is_reachable())
            .map(|overview| {
                crate::provider::eligible_new_providers(&overview.provider_capabilities)
            })
            .unwrap_or_default()
    }

    pub(in crate::navigator) fn provider_choice_is_current(
        &self,
        host: &NavigatorHost,
        provider: ProviderKind,
    ) -> bool {
        self.eligible_providers_for_host(host).contains(&provider)
    }

    pub(in crate::navigator) fn provider_choice_for_new(
        &self,
        source: &NavigatorWorkstream,
    ) -> ProviderChoice {
        let providers = self.eligible_providers_for_host(&source.host);
        match providers.as_slice() {
            [] => ProviderChoice::None,
            [provider] => ProviderChoice::Immediate(*provider),
            _ => ProviderChoice::Modal {
                selected: providers
                    .iter()
                    .position(|provider| *provider == source.provider)
                    .unwrap_or(0),
                providers,
            },
        }
    }

    pub(in crate::navigator) fn select_provider_next(&mut self) {
        let Some(NavigatorModal::SelectProvider {
            providers,
            selected,
            ..
        }) = self.modal.as_mut()
        else {
            return;
        };
        if !providers.is_empty() {
            *selected = (*selected + 1) % providers.len();
        }
    }

    pub(in crate::navigator) fn select_provider_previous(&mut self) {
        let Some(NavigatorModal::SelectProvider {
            providers,
            selected,
            ..
        }) = self.modal.as_mut()
        else {
            return;
        };
        if !providers.is_empty() {
            *selected = selected.checked_sub(1).unwrap_or(providers.len() - 1);
        }
    }

    /// Counts only Runtimes which the host registry still considers live.
    /// `Unknown` and recovery-required rows correspond to durably stopped
    /// Runtime records, so they cannot block exact observer removal.
    pub(in crate::navigator) fn live_runtime_count(&self, host_alias: &str) -> usize {
        self.snapshot
            .workstreams
            .iter()
            .filter(|workstream| {
                workstream.host.alias() == host_alias
                    && matches!(
                        workstream.runtime_status,
                        NavigatorRuntimeStatus::Starting
                            | NavigatorRuntimeStatus::Idle
                            | NavigatorRuntimeStatus::Working
                            | NavigatorRuntimeStatus::Attention
                    )
            })
            .count()
    }

    pub(in crate::navigator) fn begin_host_registration(&mut self) {
        self.modal = Some(NavigatorModal::RegisterHost {
            value: String::new(),
        });
    }

    pub(in crate::navigator) fn begin_project_browser_root_configuration(&mut self) {
        let Some(host) = self.selected_host_for_project_browser() else {
            self.set_message("no Host is selected");
            return;
        };
        self.modal = Some(NavigatorModal::ConfigureProjectBrowserRoot {
            host,
            value: "~".to_owned(),
        });
    }

    pub(in crate::navigator) fn begin_host_forget(&mut self, host: NavigatorHostSummary) {
        self.modal = Some(NavigatorModal::SelectHostRemoval {
            alias: host.alias,
            workstream_count: host.workstream_count,
            location_count: host.location_count,
            unresolved_operation_count: host.unresolved_operation_count,
            offboard: false,
        });
    }

    pub(in crate::navigator) fn begin_project_forget(&mut self) {
        let Some(project) = self.selected_project.and_then(|project_id| {
            self.projects()
                .into_iter()
                .find(|project| project.project_id == project_id)
        }) else {
            self.set_message("no Project is selected");
            return;
        };
        if project.active_workstream_count > 0 {
            self.set_message(format!(
                "archive {} active Workstream{} before removing this Project",
                project.active_workstream_count,
                if project.active_workstream_count == 1 {
                    ""
                } else {
                    "s"
                }
            ));
            return;
        }
        self.modal = Some(NavigatorModal::ConfirmForgetProject {
            project_id: project.project_id,
            label: project.label,
            archived_workstream_count: project.archived_workstream_count,
            location_count: project.locations.len(),
        });
    }

    pub(in crate::navigator) fn begin_checkout_registration(&mut self) {
        let mut hosts = self
            .snapshot
            .hosts
            .iter()
            .map(|host| {
                if host.alias == "local" {
                    NavigatorHost::Local
                } else {
                    NavigatorHost::Remote {
                        alias: host.alias.clone(),
                        reachability: host.reachability,
                    }
                }
            })
            .collect::<Vec<_>>();
        if !hosts
            .iter()
            .any(|host| matches!(host, NavigatorHost::Local))
        {
            hosts.push(NavigatorHost::Local);
        }
        hosts.sort_by(|left, right| {
            (left.alias() != "local")
                .cmp(&(right.alias() != "local"))
                .then_with(|| left.alias().cmp(right.alias()))
        });
        hosts.truncate(16);
        self.modal = Some(NavigatorModal::SelectRegistrationHost { hosts, selected: 0 });
    }

    pub(in crate::navigator) fn select_registration_host_next(&mut self) {
        let Some(NavigatorModal::SelectRegistrationHost { hosts, selected }) = self.modal.as_mut()
        else {
            return;
        };
        if !hosts.is_empty() {
            *selected = (*selected + 1) % hosts.len();
        }
    }

    pub(in crate::navigator) fn select_registration_host_previous(&mut self) {
        let Some(NavigatorModal::SelectRegistrationHost { hosts, selected }) = self.modal.as_mut()
        else {
            return;
        };
        if !hosts.is_empty() {
            *selected = selected.checked_sub(1).unwrap_or(hosts.len() - 1);
        }
    }

    pub(in crate::navigator) fn normalize_project_browser_selection(&mut self) {
        let Some(NavigatorModal::ProjectBrowser {
            directories,
            selected,
            scroll,
            filter,
            ..
        }) = self.modal.as_mut()
        else {
            return;
        };
        let visible = project_browser_entry_indexes(directories, filter);
        if visible.is_empty() {
            *selected = 0;
            *scroll = 0;
        } else if !visible.contains(selected) {
            *selected = visible[0];
        }
        project_browser_scroll_to_selected(scroll, &visible, *selected);
    }

    pub(in crate::navigator) fn select_project_browser_next(&mut self) {
        let Some(NavigatorModal::ProjectBrowser {
            directories,
            selected,
            scroll,
            filter,
            ..
        }) = self.modal.as_mut()
        else {
            return;
        };
        let visible = project_browser_entry_indexes(directories, filter);
        if let Some(position) = visible.iter().position(|index| index == selected) {
            *selected = visible[(position + 1) % visible.len()];
        } else if let Some(first) = visible.first() {
            *selected = *first;
        }
        project_browser_scroll_to_selected(scroll, &visible, *selected);
    }

    pub(in crate::navigator) fn select_project_browser_previous(&mut self) {
        let Some(NavigatorModal::ProjectBrowser {
            directories,
            selected,
            scroll,
            filter,
            ..
        }) = self.modal.as_mut()
        else {
            return;
        };
        let visible = project_browser_entry_indexes(directories, filter);
        if let Some(position) = visible.iter().position(|index| index == selected) {
            *selected = visible[(position + visible.len() - 1) % visible.len()];
        } else if let Some(last) = visible.last() {
            *selected = *last;
        }
        project_browser_scroll_to_selected(scroll, &visible, *selected);
    }

    pub(in crate::navigator) fn project_browser_selected_entry(
        &self,
    ) -> Option<(NavigatorHost, String, ProjectDirectoryEntry)> {
        let NavigatorModal::ProjectBrowser {
            host,
            directories,
            selected,
            filter,
            ..
        } = self.modal.as_ref()?
        else {
            return None;
        };
        project_browser_entry_indexes(directories, filter)
            .contains(selected)
            .then(|| {
                let entry = directories.entries.get(*selected)?.clone();
                Some((host.clone(), directories.relative_path.clone(), entry))
            })?
    }

    pub(in crate::navigator) fn project_browser_navigation_context(
        &self,
    ) -> Option<(NavigatorHost, String, bool)> {
        let NavigatorModal::ProjectBrowser {
            host,
            directories,
            include_hidden,
            ..
        } = self.modal.as_ref()?
        else {
            return None;
        };
        Some((
            host.clone(),
            directories.relative_path.clone(),
            *include_hidden,
        ))
    }

    pub(in crate::navigator) fn project_browser_selected_name(&self) -> Option<String> {
        self.project_browser_selected_entry()
            .map(|(_, _, entry)| entry.name)
    }

    pub(in crate::navigator) fn toggle_host_removal_mode(&mut self) {
        if let Some(NavigatorModal::SelectHostRemoval { offboard, .. }) = self.modal.as_mut() {
            *offboard = !*offboard;
        }
    }

    pub(in crate::navigator) fn dismiss_modal(&mut self) {
        self.modal = None;
    }

    pub(in crate::navigator) fn confirm_modal(&mut self) -> Option<NavigatorModal> {
        self.modal.take()
    }

    pub(in crate::navigator) const fn modal_visible(&self) -> bool {
        self.modal.is_some()
    }

    pub(in crate::navigator) const fn help_visible(&self) -> bool {
        self.help_visible
    }

    pub(in crate::navigator) fn scroll_help_next(&mut self) {
        let last = help_lines(
            self.page,
            self.detail.is_some(),
            matches!(self.detail, Some(NavigatorDetail::ForkRecovery { .. })),
            self.view_mode,
        )
        .len()
        .saturating_sub(1);
        self.help_scroll = self
            .help_scroll
            .saturating_add(1)
            .min(u16::try_from(last).unwrap_or(u16::MAX));
    }

    pub(in crate::navigator) fn scroll_help_previous(&mut self) {
        self.help_scroll = self.help_scroll.saturating_sub(1);
    }

    pub(in crate::navigator) fn cycle_view_mode_next(&mut self) {
        self.view_mode = self.view_mode.next();
        self.normalize_workstream_selection();
    }

    pub(in crate::navigator) fn cycle_view_mode_previous(&mut self) {
        self.view_mode = self.view_mode.previous();
        self.normalize_workstream_selection();
    }

    pub(in crate::navigator) const fn view_mode(&self) -> NavigatorViewMode {
        self.view_mode
    }

    pub fn render(&mut self, frame: &mut Frame<'_>) {
        let status = self.footer_status();
        let status_height = if status.is_empty() {
            0
        } else {
            STATUS_BOX_HEIGHT
        };
        let help_height = if self.help_visible {
            frame.area().height.saturating_sub(4).clamp(3, 22)
        } else {
            2
        };
        let areas = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(3),
                Constraint::Length(status_height),
                Constraint::Length(help_height),
            ])
            .split(frame.area());
        let content_area = if self.page == NavigatorPage::Workstreams {
            areas[0]
        } else {
            self.render_workstreams_parent(frame, areas[0])
        };
        match self.detail.clone() {
            Some(NavigatorDetail::ForkRecovery { .. }) => {
                self.render_fork_recovery_detail(frame, content_area);
            }
            Some(NavigatorDetail::Workstream {
                host_alias,
                workstream_id,
            }) => self.render_workstream_detail(frame, content_area, &host_alias, workstream_id),
            None => match self.page {
                NavigatorPage::Workstreams => self.render_workstreams(frame, content_area),
                NavigatorPage::Projects => self.render_projects(frame, content_area),
                NavigatorPage::Hosts => self.render_hosts(frame, content_area),
            },
        }
        if !status.is_empty() {
            self.render_status_box(frame, areas[1], &status);
        }
        if self.help_visible {
            self.render_help_reference(frame, areas[2]);
        } else {
            frame.render_widget(
                Paragraph::new(self.compact_key_lines(areas[2].width))
                    .style(Style::default().fg(Color::Gray)),
                areas[2],
            );
        }
        if let Some(modal) = self.modal.clone() {
            Self::render_modal(frame, modal);
        }
    }

    /// Renders the structural parent for infrequent management surfaces without
    /// turning the navigator into a tabbed dashboard. The child block starts
    /// on the next row, so it reads as a temporary page floating over the
    /// Workstreams home rather than a sibling view.
    fn render_workstreams_parent(&self, frame: &mut Frame<'_>, area: Rect) -> Rect {
        if area.height < 4 {
            return area;
        }
        let parent = Rect::new(area.x, area.y, area.width, 1);
        frame.render_widget(
            Block::default()
                .borders(Borders::TOP | Borders::LEFT | Borders::RIGHT)
                .title(Line::from(Span::styled(
                    format!(" Workstreams · {} ", self.view_mode().label()),
                    Style::default().fg(Color::Gray),
                )))
                .border_style(Style::default().fg(Color::Gray)),
            parent,
        );
        Rect::new(
            area.x,
            area.y.saturating_add(1),
            area.width,
            area.height.saturating_sub(1),
        )
    }

    fn render_workstreams(&mut self, frame: &mut Frame<'_>, area: Rect) {
        let entries = self.list_entries();
        let project_colors = visible_project_colors(&self.snapshot);
        let items = entries
            .iter()
            .map(|entry| {
                navigator_list_item(
                    entry,
                    &self.snapshot,
                    &project_colors,
                    area.width.saturating_sub(2),
                )
            })
            .collect::<Vec<_>>();
        let mut state = ListState::default();
        state.select(
            entries
                .iter()
                .position(|entry| entry.workstream_index() == Some(self.selected)),
        );
        frame.render_stateful_widget(
            List::new(items)
                .block(Block::default().borders(Borders::ALL).title(format!(
                    " {} · {} ",
                    "Workstreams",
                    self.view_mode().label()
                )))
                .highlight_style(selected_row_style()),
            area,
            &mut state,
        );
        self.rendered_offset = state.offset();
        self.update_rendered_mouse_rows(&entries, area);
    }

    fn render_projects(&mut self, frame: &mut Frame<'_>, area: Rect) {
        let projects = self.projects();
        let project_colors = visible_project_colors(&self.snapshot);
        let items = projects
            .iter()
            .map(|project| {
                project_overview_item(project, &project_colors, area.width.saturating_sub(2))
            })
            .collect::<Vec<_>>();
        let mut state = ListState::default();
        state.select(
            projects
                .iter()
                .position(|project| Some(project.project_id) == self.selected_project),
        );
        frame.render_stateful_widget(
            List::new(items)
                .block(Block::default().borders(Borders::ALL).title(" Projects "))
                .highlight_style(selected_row_style()),
            area,
            &mut state,
        );
        self.update_rendered_project_rows(&projects, state.offset(), area);
    }

    fn render_hosts(&mut self, frame: &mut Frame<'_>, area: Rect) {
        let hosts = self.hosts();
        let project_colors = visible_project_colors(&self.snapshot);
        let items = hosts
            .iter()
            .map(|host| host_overview_item(host, &project_colors, area.width.saturating_sub(2)))
            .collect::<Vec<_>>();
        let mut state = ListState::default();
        state.select(
            hosts
                .iter()
                .position(|host| Some(host.alias.as_str()) == self.selected_host.as_deref()),
        );
        frame.render_stateful_widget(
            List::new(items)
                .block(Block::default().borders(Borders::ALL).title(" Hosts "))
                .highlight_style(selected_row_style()),
            area,
            &mut state,
        );
        self.update_rendered_host_rows(&hosts, state.offset(), area);
    }

    fn render_workstream_detail(
        &self,
        frame: &mut Frame<'_>,
        area: Rect,
        host_alias: &str,
        workstream_id: WorkstreamId,
    ) {
        let Some(workstream) = self.snapshot.workstreams.iter().find(|workstream| {
            workstream.host.alias() == host_alias && workstream.workstream_id == workstream_id
        }) else {
            return;
        };
        let attention = if workstream.recovery_required {
            "native recovery needed"
        } else if workstream.result_ready {
            "result ready"
        } else {
            "none"
        };
        let visibility = if workstream.archived {
            "archived"
        } else {
            "active"
        };
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(Span::styled(
                    workstream.display_name.clone(),
                    Style::default().add_modifier(Modifier::BOLD),
                )),
                Line::raw(format!(
                    "{} · {}",
                    workstream.host.alias(),
                    workstream.project_label
                )),
                Line::raw(format!("Provider: {}", provider_label(workstream.provider))),
                Line::raw(format!("Runtime: {}", workstream.runtime_status.label())),
                Line::raw(format!("Attention: {attention}")),
                Line::raw(if workstream.archived {
                    format!(
                        "Visibility: {visibility}; restore does not start {}",
                        provider_label(workstream.provider)
                    )
                } else {
                    format!("Visibility: {visibility}")
                }),
                Line::raw("Enter or Esc returns to Workstreams"),
            ])
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Workstream status "),
            ),
            area,
        );
    }

    fn render_fork_recovery_detail(&self, frame: &mut Frame<'_>, area: Rect) {
        let lines = self
            .fork_recovery_operations()
            .into_iter()
            .enumerate()
            .map(|(index, operation)| {
                let marker = if index == self.selected_operation {
                    "> "
                } else {
                    "  "
                };
                Line::from(Span::styled(
                    format!(
                        "{marker}{} · {} · {}",
                        operation.host.alias(),
                        operation_kind_label(operation.kind),
                        operation_phase_label(operation.phase)
                    ),
                    if index == self.selected_operation {
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::Gray)
                    },
                ))
            })
            .collect::<Vec<_>>();
        frame.render_widget(
            Paragraph::new(lines).block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Unfinished forks · Enter reconcile · Esc back "),
            ),
            area,
        );
    }

    pub(in crate::navigator) fn list_entries(&self) -> Vec<NavigatorListEntry> {
        match self.view_mode {
            NavigatorViewMode::Recent => self
                .snapshot
                .workstreams
                .iter()
                .enumerate()
                .filter(|(_, row)| self.view_mode.includes(row))
                .map(|(snapshot_index, _)| NavigatorListEntry::Workstream {
                    snapshot_index,
                    context: WorkstreamRowContext::Recent,
                    tree_branch: None,
                })
                .collect(),
            NavigatorViewMode::Host => {
                let mut groups = Vec::<(String, Vec<usize>)>::new();
                for (snapshot_index, row) in self.snapshot.workstreams.iter().enumerate() {
                    if !self.view_mode.includes(row) {
                        continue;
                    }
                    if let Some((_, indexes)) = groups
                        .iter_mut()
                        .find(|(alias, _)| alias == row.host.alias())
                    {
                        indexes.push(snapshot_index);
                    } else {
                        groups.push((row.host.alias().to_owned(), vec![snapshot_index]));
                    }
                }
                groups
                    .into_iter()
                    .flat_map(|(alias, indexes)| {
                        let count = indexes.len();
                        std::iter::once(NavigatorListEntry::HostHeader { alias }).chain(
                            indexes
                                .into_iter()
                                .enumerate()
                                .map(move |(index, snapshot_index)| {
                                    NavigatorListEntry::Workstream {
                                        snapshot_index,
                                        context: WorkstreamRowContext::Host,
                                        tree_branch: Some(TreeBranch {
                                            is_last: index + 1 == count,
                                        }),
                                    }
                                }),
                        )
                    })
                    .collect()
            }
            NavigatorViewMode::Project => {
                let mut groups = Vec::<(ProjectId, String, Vec<usize>)>::new();
                for (snapshot_index, row) in self.snapshot.workstreams.iter().enumerate() {
                    if !self.view_mode.includes(row) {
                        continue;
                    }
                    if let Some((_, _, indexes)) = groups
                        .iter_mut()
                        .find(|(project_id, _, _)| *project_id == row.project_id)
                    {
                        indexes.push(snapshot_index);
                    } else {
                        groups.push((
                            row.project_id,
                            row.project_label.clone(),
                            vec![snapshot_index],
                        ));
                    }
                }
                groups
                    .into_iter()
                    .flat_map(|(project_id, label, indexes)| {
                        let count = indexes.len();
                        std::iter::once(NavigatorListEntry::ProjectHeader { project_id, label })
                            .chain(indexes.into_iter().enumerate().map(
                                move |(index, snapshot_index)| NavigatorListEntry::Workstream {
                                    snapshot_index,
                                    context: WorkstreamRowContext::Project,
                                    tree_branch: Some(TreeBranch {
                                        is_last: index + 1 == count,
                                    }),
                                },
                            ))
                    })
                    .collect()
            }
            NavigatorViewMode::Archived => self
                .snapshot
                .workstreams
                .iter()
                .enumerate()
                .filter(|(_, row)| self.view_mode.includes(row))
                .map(|(snapshot_index, _)| NavigatorListEntry::Workstream {
                    snapshot_index,
                    context: WorkstreamRowContext::Archived,
                    tree_branch: None,
                })
                .collect(),
        }
    }

    fn update_rendered_mouse_rows(&mut self, entries: &[NavigatorListEntry], area: Rect) {
        self.rendered_mouse_rows.clear();
        let content_top = area.y.saturating_add(1);
        let content_bottom = area.y.saturating_add(area.height.saturating_sub(1));
        let mut y = content_top;
        for entry in entries.iter().skip(self.rendered_offset) {
            if y >= content_bottom {
                break;
            }
            let next_y = y.saturating_add(entry.height()).min(content_bottom);
            if let Some(snapshot_index) = entry.workstream_index() {
                self.rendered_mouse_rows
                    .extend((y..next_y).map(|row_y| (row_y, snapshot_index)));
            }
            y = next_y;
        }
    }

    fn update_rendered_project_rows(
        &mut self,
        projects: &[NavigatorProjectOverview],
        offset: usize,
        area: Rect,
    ) {
        self.rendered_project_rows.clear();
        let content_top = area.y.saturating_add(1);
        let content_bottom = area.y.saturating_add(area.height.saturating_sub(1));
        let mut y = content_top;
        for project in projects.iter().skip(offset) {
            if y >= content_bottom {
                break;
            }
            let next_y = y
                .saturating_add(project_overview_height(project))
                .min(content_bottom);
            self.rendered_project_rows
                .extend((y..next_y).map(|row_y| (row_y, project.project_id)));
            y = next_y;
        }
    }

    fn update_rendered_host_rows(
        &mut self,
        hosts: &[NavigatorHostSummary],
        offset: usize,
        area: Rect,
    ) {
        self.rendered_host_rows.clear();
        let content_top = area.y.saturating_add(1);
        let content_bottom = area.y.saturating_add(area.height.saturating_sub(1));
        let mut y = content_top;
        for host in hosts.iter().skip(offset) {
            if y >= content_bottom {
                break;
            }
            let next_y = y
                .saturating_add(host_overview_height(host))
                .min(content_bottom);
            self.rendered_host_rows
                .extend((y..next_y).map(|row_y| (row_y, host.alias.clone())));
            y = next_y;
        }
    }

    pub(in crate::navigator) fn project_from_y(&self, y: u16) -> Option<ProjectId> {
        self.rendered_project_rows
            .iter()
            .find_map(|(row_y, project_id)| (*row_y == y).then_some(*project_id))
    }

    pub(in crate::navigator) fn host_from_y(&self, y: u16) -> Option<String> {
        self.rendered_host_rows
            .iter()
            .find_map(|(row_y, alias)| (*row_y == y).then_some(alias.clone()))
    }

    pub(in crate::navigator) fn begin_project_click(&mut self, target: Option<ProjectId>) {
        self.mouse_click = Some(if target.is_some() {
            MouseClickIntent::Management
        } else {
            MouseClickIntent::Blank
        });
        if let Some(project_id) = target {
            self.selected_project = Some(project_id);
        }
    }

    pub(in crate::navigator) fn begin_host_click(&mut self, alias: Option<String>) {
        self.mouse_click = Some(if alias.is_some() {
            MouseClickIntent::Management
        } else {
            MouseClickIntent::Blank
        });
        if let Some(alias) = alias {
            self.selected_host = Some(alias);
        }
    }

    pub(in crate::navigator) fn footer_status(&self) -> String {
        if let Some((message, _)) = &self.transient_message {
            return message.clone();
        }
        if let Some(message) = &self.message {
            return message.clone();
        }
        let operation_hint = (self.snapshot.unresolved_operation_count > 0).then(|| {
            format!(
                "! {} unfinished Fork{}; press f on its source",
                self.snapshot.unresolved_operation_count,
                if self.snapshot.unresolved_operation_count == 1 {
                    ""
                } else {
                    "s"
                }
            )
        });
        if self.visible_workstream_indexes().is_empty() {
            let empty_label = if self.snapshot.workstreams.is_empty() {
                "No Workstreams yet · n adds a Project".to_owned()
            } else if self.view_mode.is_archived() {
                "No archived Workstreams".to_owned()
            } else {
                "No active Workstreams".to_owned()
            };
            return format!(
                "{empty_label}{}",
                operation_hint.map_or_else(String::new, |hint| format!("  ·  {hint}"))
            );
        }
        let cached_hint = self.cached_remote_hint();
        match (operation_hint, cached_hint) {
            (Some(operation), Some(cached)) => format!("{operation}  ·  {cached}"),
            (Some(operation), None) => operation,
            (None, Some(cached)) => cached,
            (None, None) => String::new(),
        }
    }

    fn cached_remote_hint(&self) -> Option<String> {
        let unavailable = self
            .hosts()
            .into_iter()
            .filter_map(|host| match host.reachability {
                RemoteHostReachability::Reachable => None,
                RemoteHostReachability::Unreachable(issue) => {
                    Some(format!("{} {}", host.alias, issue.label()))
                }
            })
            .collect::<Vec<_>>();
        (!unavailable.is_empty())
            .then(|| format!("{}; showing cached state", unavailable.join(", ")))
    }

    pub(in crate::navigator) fn compact_key_lines(&self, width: u16) -> Vec<Line<'static>> {
        if width < 16 {
            return vec![binding_line(&[("?", "keys")])];
        }
        let bindings = self.compact_bindings();
        let mut rows = vec![Vec::<(&str, &str)>::new()];
        let mut row_width = 0_usize;
        let maximum = usize::from(width).saturating_sub(COMPACT_HINT_LEFT_INSET);
        for binding in bindings {
            let binding_width = binding.0.len() + 1 + binding.1.len();
            let separator_width = usize::from(!rows.last().unwrap().is_empty()) * 2;
            if row_width + separator_width + binding_width > maximum {
                if rows.len() == 2 {
                    break;
                }
                rows.push(Vec::new());
                row_width = 0;
            }
            if !rows.last().unwrap().is_empty() {
                row_width += 2;
            }
            row_width += binding_width;
            rows.last_mut().unwrap().push(binding);
        }
        rows.into_iter()
            .filter(|row| !row.is_empty())
            .map(|row| binding_line(&row))
            .collect()
    }

    fn compact_bindings(&self) -> Vec<(&'static str, &'static str)> {
        if self.detail.is_some() {
            return vec![("?", "keys")];
        }
        match self.page {
            NavigatorPage::Workstreams => {
                let mut bindings = vec![("←/→", "view")];
                if self.view_mode.is_archived() {
                    bindings.push(("u", "restore"));
                } else {
                    bindings.extend([
                        (
                            "n",
                            if self.snapshot.workstreams.is_empty() {
                                "register"
                            } else {
                                "new"
                            },
                        ),
                        ("f", "fork"),
                        ("p", "park"),
                        ("r", "rename"),
                        ("a", "ack"),
                        ("x", "archive"),
                    ]);
                }
                bindings.extend([("i", "status"), ("?", "keys")]);
                bindings
            }
            NavigatorPage::Projects => vec![
                ("a", "add"),
                ("x", "remove"),
                (",", "workstreams"),
                (".", "hosts"),
                ("?", "keys"),
            ],
            NavigatorPage::Hosts => vec![
                ("a", "add"),
                ("s", "Codex observer"),
                ("x", "remove"),
                ("r", "root"),
                (",", "projects"),
                (".", "workstreams"),
                ("?", "keys"),
            ],
        }
    }

    fn footer_style(&self) -> Style {
        if self.help_visible {
            Style::default().fg(Color::Cyan)
        } else if self.message.is_some()
            || self.transient_message.is_some()
            || self.snapshot.unresolved_operation_count > 0
            || !self.snapshot.unreachable_hosts.is_empty()
        {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default().fg(Color::Gray)
        }
    }

    fn render_status_box(&self, frame: &mut Frame<'_>, area: Rect, status: &str) {
        frame.render_widget(
            Paragraph::new(status)
                .style(self.footer_style())
                .wrap(Wrap { trim: true })
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(" Status ")
                        .border_style(self.footer_style()),
                ),
            area,
        );
    }

    fn render_help_reference(&self, frame: &mut Frame<'_>, area: Rect) {
        frame.render_widget(
            Paragraph::new(help_lines(
                self.page,
                self.detail.is_some(),
                matches!(self.detail, Some(NavigatorDetail::ForkRecovery { .. })),
                self.view_mode,
            ))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(format!(" Keys · {} ", self.page.label()))
                    .border_style(Style::default().fg(Color::Cyan)),
            )
            .scroll((self.help_scroll, 0)),
            area,
        );
    }

    fn render_modal(frame: &mut Frame<'_>, modal: NavigatorModal) {
        let area = navigator_modal_area(frame.area(), &modal);
        let (title, lines) =
            navigator_modal_content(modal, usize::from(area.width.saturating_sub(2)));
        frame.render_widget(Clear, area);
        frame.render_widget(
            Paragraph::new(lines).wrap(Wrap { trim: false }).block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(title)
                    .border_style(Style::default().fg(Color::Yellow)),
            ),
            area,
        );
    }

    #[must_use]
    pub fn row_from_y(&self, y: u16) -> Option<usize> {
        self.rendered_mouse_rows
            .iter()
            .find_map(|(row_y, snapshot_index)| (*row_y == y).then_some(*snapshot_index))
    }
}
