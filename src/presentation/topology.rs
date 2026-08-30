use super::{PresentationError, PresentationPaneRole, TMUX_FIELD_SEPARATOR, WorkstreamId};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Direction {
    Up,
    Down,
    Left,
    Right,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct OwnedPane {
    pub(crate) id: String,
    pub(crate) role: PresentationPaneRole,
    pub(crate) workstream_id: Option<WorkstreamId>,
    pub(crate) dead: bool,
    pub(crate) left: u16,
    pub(crate) top: u16,
    pub(crate) width: u16,
    pub(crate) height: u16,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PresentationTopology {
    pub(crate) panes: Vec<OwnedPane>,
    pub(crate) window_width: u16,
    pub(crate) window_height: u16,
}

impl PresentationTopology {
    pub(crate) fn pane(&self, id: &str) -> Option<&OwnedPane> {
        self.panes.iter().find(|pane| pane.id == id)
    }

    pub(crate) fn navigator(&self) -> Option<&OwnedPane> {
        self.panes
            .iter()
            .find(|pane| pane.role == PresentationPaneRole::Navigator)
    }

    pub(crate) fn provider(&self) -> Option<&OwnedPane> {
        self.panes
            .iter()
            .find(|pane| pane.role == PresentationPaneRole::Provider)
    }

    pub(crate) fn utility(&self) -> Option<&OwnedPane> {
        self.panes
            .iter()
            .find(|pane| pane.role == PresentationPaneRole::Utility)
    }

    pub(crate) fn next(&self, source: &OwnedPane) -> Option<&OwnedPane> {
        let mut panes: Vec<&OwnedPane> = self.panes.iter().collect();
        panes.sort_by_key(|pane| (pane.top, pane.left, pane.id.as_str()));
        let index = panes.iter().position(|pane| pane.id == source.id)?;
        panes.get((index + 1) % panes.len()).copied()
    }

    pub(crate) fn directional(
        &self,
        source: &OwnedPane,
        direction: Direction,
    ) -> Option<&OwnedPane> {
        let source_x = i32::from(source.left) + i32::from(source.width) / 2;
        let source_y = i32::from(source.top) + i32::from(source.height) / 2;
        let mut candidates: Vec<(&OwnedPane, (i32, i32))> = self
            .panes
            .iter()
            .filter(|pane| pane.id != source.id)
            .filter_map(|pane| {
                let pane_x = i32::from(pane.left) + i32::from(pane.width) / 2;
                let pane_y = i32::from(pane.top) + i32::from(pane.height) / 2;
                let (primary, secondary) = match direction {
                    Direction::Up if pane_y < source_y => {
                        (source_y - pane_y, (source_x - pane_x).abs())
                    }
                    Direction::Down if pane_y > source_y => {
                        (pane_y - source_y, (source_x - pane_x).abs())
                    }
                    Direction::Left if pane_x < source_x => {
                        (source_x - pane_x, (source_y - pane_y).abs())
                    }
                    Direction::Right if pane_x > source_x => {
                        (pane_x - source_x, (source_y - pane_y).abs())
                    }
                    _ => return None,
                };
                Some((pane, (primary, secondary)))
            })
            .collect();
        candidates.sort_by_key(|(pane, distance)| (*distance, pane.id.as_str()));
        candidates.first().map(|(pane, _)| *pane)
    }
}

pub(crate) fn parse_topology(output: &str) -> Result<PresentationTopology, PresentationError> {
    parse_topology_with_dead(output, false)
}

pub(crate) fn parse_topology_with_dead(
    output: &str,
    allow_dead: bool,
) -> Result<PresentationTopology, PresentationError> {
    let mut panes = Vec::new();
    let mut window_size = None;
    for line in output.lines() {
        panes.push(parse_topology_line(
            line,
            allow_dead,
            &mut window_size,
            &panes,
        )?);
    }
    if !(2..=3).contains(&panes.len()) {
        return Err(PresentationError::InvalidTopology);
    }
    let (window_width, window_height) = window_size.ok_or(PresentationError::InvalidTopology)?;
    let topology = PresentationTopology {
        panes,
        window_width,
        window_height,
    };
    validate_topology_shape(&topology)?;
    Ok(topology)
}

fn parse_topology_line(
    line: &str,
    allow_dead: bool,
    window_size: &mut Option<(u16, u16)>,
    panes: &[OwnedPane],
) -> Result<OwnedPane, PresentationError> {
    if line.is_empty() {
        return Err(PresentationError::InvalidTopology);
    }
    let fields: Vec<&str> = line.split(TMUX_FIELD_SEPARATOR).collect();
    if fields.len() != 10 {
        return Err(PresentationError::InvalidTopology);
    }
    let id = parse_pane_id(fields[0]).ok_or(PresentationError::InvalidTopology)?;
    if panes.iter().any(|pane| pane.id == id) {
        return Err(PresentationError::InvalidTopology);
    }
    let role = match fields[1] {
        "navigator" => PresentationPaneRole::Navigator,
        "provider" => PresentationPaneRole::Provider,
        "utility" => PresentationPaneRole::Utility,
        _ => return Err(PresentationError::InvalidTopology),
    };
    let workstream_id = if fields[2].is_empty() {
        None
    } else {
        Some(
            fields[2]
                .parse()
                .map_err(|_| PresentationError::InvalidTopology)?,
        )
    };
    if (role == PresentationPaneRole::Navigator && workstream_id.is_some())
        || (role == PresentationPaneRole::Utility && workstream_id.is_none())
    {
        return Err(PresentationError::InvalidTopology);
    }
    let dead = match fields[3] {
        "0" => false,
        "1" if allow_dead => true,
        _ => return Err(PresentationError::InvalidTopology),
    };
    let window_width = topology_dimension(fields[8])?;
    let window_height = topology_dimension(fields[9])?;
    if window_width == 0 || window_height == 0 {
        return Err(PresentationError::InvalidTopology);
    }
    if let Some((expected_width, expected_height)) = window_size {
        if (*expected_width, *expected_height) != (window_width, window_height) {
            return Err(PresentationError::InvalidTopology);
        }
    } else {
        *window_size = Some((window_width, window_height));
    }
    let left = topology_dimension(fields[4])?;
    let top = topology_dimension(fields[5])?;
    let width = topology_dimension(fields[6])?;
    let height = topology_dimension(fields[7])?;
    if width == 0
        || height == 0
        || u32::from(left) + u32::from(width) > u32::from(window_width)
        || u32::from(top) + u32::from(height) > u32::from(window_height)
    {
        return Err(PresentationError::InvalidTopology);
    }
    Ok(OwnedPane {
        id,
        role,
        workstream_id,
        dead,
        left,
        top,
        width,
        height,
    })
}

fn topology_dimension(value: &str) -> Result<u16, PresentationError> {
    value
        .parse::<u16>()
        .map_err(|_| PresentationError::InvalidTopology)
}

fn validate_topology_shape(topology: &PresentationTopology) -> Result<(), PresentationError> {
    if topology.navigator().is_none()
        || topology.provider().is_none()
        || topology
            .panes
            .iter()
            .filter(|pane| pane.role == PresentationPaneRole::Navigator)
            .count()
            != 1
        || topology
            .panes
            .iter()
            .filter(|pane| pane.role == PresentationPaneRole::Provider)
            .count()
            != 1
        || topology
            .panes
            .iter()
            .filter(|pane| pane.role == PresentationPaneRole::Utility)
            .count()
            > 1
    {
        return Err(PresentationError::InvalidTopology);
    }
    let navigator = topology
        .navigator()
        .ok_or(PresentationError::InvalidTopology)?;
    let provider = topology
        .provider()
        .ok_or(PresentationError::InvalidTopology)?;
    if navigator.left != 0
        || navigator.top != 0
        || navigator.height != topology.window_height
        || provider.top != 0
        || provider.left
            != navigator
                .left
                .saturating_add(navigator.width)
                .saturating_add(1)
        || provider.left <= navigator.left
        || u32::from(provider.left) + u32::from(provider.width) != u32::from(topology.window_width)
    {
        return Err(PresentationError::InvalidTopology);
    }
    match topology.utility() {
        None if provider.height == topology.window_height => {}
        Some(utility)
            if provider.height < topology.window_height
                && utility.left == provider.left
                && utility.width == provider.width
                && u32::from(utility.top)
                    == u32::from(provider.top) + u32::from(provider.height) + 1
                && u32::from(utility.top) + u32::from(utility.height)
                    == u32::from(topology.window_height) => {}
        _ => return Err(PresentationError::InvalidTopology),
    }
    Ok(())
}

fn parse_pane_id(value: &str) -> Option<String> {
    value
        .strip_prefix('%')
        .filter(|digits| {
            !digits.is_empty() && digits.chars().all(|character| character.is_ascii_digit())
        })
        .map(|_| value.to_owned())
}
