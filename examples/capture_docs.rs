//! Generates privacy-safe product captures from the real navigator renderer.
//!
//! The fixture contains fixed safe labels and no host paths, provider IDs,
//! prompts, responses, or user-owned content. Run `scripts/capture-docs` from
//! the repository root to refresh the committed SVG and PNG captures.

use std::{
    error::Error,
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use ratatui::{
    Terminal,
    backend::TestBackend,
    buffer::Buffer,
    style::{Color, Modifier},
};
use uuid::Uuid;
use wsnav::{
    domain::{LocationId, ProjectId, Revision, WorkstreamId},
    navigator::{
        LocalNavigatorSnapshot, NavigatorHost, NavigatorHostOverview, NavigatorRuntimeStatus,
        NavigatorView, NavigatorWorkstream, RemoteHostReachability,
    },
    protocol::ObserverStatus,
};

const WIDTH: u16 = 42;
const HEIGHT: u16 = 18;
const CELL_WIDTH: u16 = 12;
const CELL_HEIGHT: u16 = 21;
const MARGIN: u16 = 18;
const TERMINAL_BACKGROUND: &str = "#101418";

type CaptureResult<T> = Result<T, Box<dyn Error>>;

fn main() -> CaptureResult<()> {
    let output_root = PathBuf::from("docs/media/screenshots");
    fs::create_dir_all(&output_root)?;

    for (name, snapshot) in [
        ("workstreams", workstreams_snapshot()),
        ("remote-recovery", recovery_snapshot()),
        ("first-project", first_project_snapshot()),
    ] {
        let mut terminal = Terminal::new(TestBackend::new(WIDTH, HEIGHT))?;
        let mut view = NavigatorView::new(snapshot);
        terminal.draw(|frame| view.render(frame))?;
        write_svg(
            &output_root.join(format!("{name}.svg")),
            terminal.backend().buffer(),
        )?;
    }

    Ok(())
}

fn workstreams_snapshot() -> LocalNavigatorSnapshot {
    let now = now_millis();
    let navigator = project(1);
    let notes = project(2);
    let website = project(3);
    LocalNavigatorSnapshot {
        workstreams: vec![
            workstream(WorkstreamCapture {
                value: 101,
                project_id: navigator,
                host: NavigatorHost::Local,
                project_label: "workstream-navigator",
                display_name: "Polish the navigator presentation",
                runtime_status: NavigatorRuntimeStatus::Working,
                result_ready: false,
                last_activity_at_millis: now.saturating_sub(15_000),
            }),
            workstream(WorkstreamCapture {
                value: 102,
                project_id: notes,
                host: remote_host("snap"),
                project_label: "release-notes",
                display_name: "Review the remote release notes",
                runtime_status: NavigatorRuntimeStatus::Attention,
                result_ready: true,
                last_activity_at_millis: now.saturating_sub(8 * 60_000),
            }),
            workstream(WorkstreamCapture {
                value: 103,
                project_id: website,
                host: NavigatorHost::Local,
                project_label: "project-site",
                display_name: "Explore a parallel direction",
                runtime_status: NavigatorRuntimeStatus::Idle,
                result_ready: false,
                last_activity_at_millis: now.saturating_sub(3 * 60 * 60_000),
            }),
        ],
        hosts: ready_hosts(),
        ..LocalNavigatorSnapshot::default()
    }
}

fn recovery_snapshot() -> LocalNavigatorSnapshot {
    let now = now_millis();
    let navigator = project(1);
    let website = project(3);
    LocalNavigatorSnapshot {
        workstreams: vec![
            workstream(WorkstreamCapture {
                value: 201,
                project_id: navigator,
                host: NavigatorHost::Local,
                project_label: "workstream-navigator",
                display_name: "Native session needs recovery",
                runtime_status: NavigatorRuntimeStatus::RecoveryRequired,
                result_ready: false,
                last_activity_at_millis: now.saturating_sub(4 * 60_000),
            }),
            workstream(WorkstreamCapture {
                value: 202,
                project_id: website,
                host: remote_host("snap"),
                project_label: "project-site",
                display_name: "Remote host is temporarily unavailable",
                runtime_status: NavigatorRuntimeStatus::Unknown,
                result_ready: false,
                last_activity_at_millis: now.saturating_sub(2 * 24 * 60 * 60_000),
            }),
        ],
        hosts: ready_hosts(),
        unreachable_hosts: vec!["snap".to_owned()],
        ..LocalNavigatorSnapshot::default()
    }
}

fn first_project_snapshot() -> LocalNavigatorSnapshot {
    LocalNavigatorSnapshot {
        hosts: ready_hosts(),
        ..LocalNavigatorSnapshot::default()
    }
}

fn project(value: u128) -> ProjectId {
    ProjectId::from(Uuid::from_u128(value))
}

fn remote_host(alias: &str) -> NavigatorHost {
    NavigatorHost::Remote {
        alias: alias.to_owned(),
        reachability: RemoteHostReachability::Reachable,
    }
}

struct WorkstreamCapture<'a> {
    value: u128,
    project_id: ProjectId,
    host: NavigatorHost,
    project_label: &'a str,
    display_name: &'a str,
    runtime_status: NavigatorRuntimeStatus,
    result_ready: bool,
    last_activity_at_millis: i64,
}

fn workstream(capture: WorkstreamCapture<'_>) -> NavigatorWorkstream {
    NavigatorWorkstream {
        host: capture.host,
        project_id: capture.project_id,
        location_id: LocationId::from(Uuid::from_u128(capture.value + 1_000)),
        workstream_id: WorkstreamId::from(Uuid::from_u128(capture.value + 2_000)),
        project_label: capture.project_label.to_owned(),
        remote_identity_display: None,
        location_label: capture.project_label.to_owned(),
        display_name: capture.display_name.to_owned(),
        runtime_status: capture.runtime_status,
        archived: false,
        result_ready: capture.result_ready,
        recovery_required: capture.runtime_status == NavigatorRuntimeStatus::RecoveryRequired,
        attention_revision: capture.result_ready.then_some(Revision::INITIAL),
        last_activity_at_millis: Some(capture.last_activity_at_millis),
        workstream_revision: Revision::INITIAL,
    }
}

fn ready_hosts() -> Vec<NavigatorHostOverview> {
    vec![
        NavigatorHostOverview {
            alias: "local".to_owned(),
            reachability: RemoteHostReachability::Reachable,
            observer_status: ObserverStatus::Ready,
        },
        NavigatorHostOverview {
            alias: "snap".to_owned(),
            reachability: RemoteHostReachability::Reachable,
            observer_status: ObserverStatus::Ready,
        },
    ]
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

fn write_svg(path: &Path, buffer: &Buffer) -> CaptureResult<()> {
    let pixel_width = WIDTH * CELL_WIDTH + MARGIN * 2;
    let pixel_height = HEIGHT * CELL_HEIGHT + MARGIN * 2;
    let mut svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{pixel_width}\" height=\"{pixel_height}\" viewBox=\"0 0 {pixel_width} {pixel_height}\" role=\"img\" aria-label=\"Workstream Navigator capture\">\n<rect width=\"100%\" height=\"100%\" rx=\"12\" fill=\"{TERMINAL_BACKGROUND}\"/>\n"
    );

    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let cell = &buffer[(x, y)];
            let pixel_x = MARGIN + x * CELL_WIDTH;
            let pixel_y = MARGIN + y * CELL_HEIGHT;
            if let Some(background) = color_css(cell.bg) {
                writeln!(
                    svg,
                    "<rect x=\"{pixel_x}\" y=\"{pixel_y}\" width=\"{CELL_WIDTH}\" height=\"{CELL_HEIGHT}\" fill=\"{background}\"/>"
                )?;
            }
            let symbol = cell.symbol();
            if symbol.trim().is_empty() {
                continue;
            }
            let color = color_css(cell.fg).unwrap_or_else(|| "#d7dde8".to_owned());
            let weight = if cell.modifier.contains(Modifier::BOLD) {
                " font-weight=\"700\""
            } else {
                ""
            };
            let baseline = pixel_y + 16;
            writeln!(
                svg,
                "<text x=\"{pixel_x}\" y=\"{baseline}\" fill=\"{color}\" font-family=\"JetBrains Mono, Iosevka, ui-monospace, monospace\" font-size=\"17\"{weight}>{}</text>",
                escape_xml(symbol)
            )?;
        }
    }
    svg.push_str("</svg>\n");
    fs::write(path, svg)?;
    Ok(())
}

fn color_css(color: Color) -> Option<String> {
    match color {
        Color::Reset => None,
        Color::Black => Some("#000000".to_owned()),
        Color::Red => Some("#cd3131".to_owned()),
        Color::Green => Some("#0dbc79".to_owned()),
        Color::Yellow => Some("#e5e510".to_owned()),
        Color::Blue => Some("#2472c8".to_owned()),
        Color::Magenta => Some("#bc3fbc".to_owned()),
        Color::Cyan => Some("#11a8cd".to_owned()),
        Color::Gray => Some("#b8b8b8".to_owned()),
        Color::DarkGray => Some("#666666".to_owned()),
        Color::LightRed => Some("#f14c4c".to_owned()),
        Color::LightGreen => Some("#23d18b".to_owned()),
        Color::LightYellow => Some("#f5f543".to_owned()),
        Color::LightBlue => Some("#3b8eea".to_owned()),
        Color::LightMagenta => Some("#d86bd8".to_owned()),
        Color::LightCyan => Some("#29b8db".to_owned()),
        Color::White => Some("#e5e5e5".to_owned()),
        Color::Indexed(index) => Some(indexed_color_css(index)),
        Color::Rgb(red, green, blue) => Some(format!("#{red:02x}{green:02x}{blue:02x}")),
    }
}

fn indexed_color_css(index: u8) -> String {
    const ANSI: [&str; 16] = [
        "#000000", "#cd3131", "#0dbc79", "#e5e510", "#2472c8", "#bc3fbc", "#11a8cd", "#e5e5e5",
        "#666666", "#f14c4c", "#23d18b", "#f5f543", "#3b8eea", "#d86bd8", "#29b8db", "#ffffff",
    ];
    if index < 16 {
        return ANSI[usize::from(index)].to_owned();
    }
    if index <= 231 {
        let value = index - 16;
        let red = value / 36;
        let green = (value / 6) % 6;
        let blue = value % 6;
        let component = |channel: u8| if channel == 0 { 0 } else { 55 + channel * 40 };
        return format!(
            "#{:02x}{:02x}{:02x}",
            component(red),
            component(green),
            component(blue)
        );
    }
    let gray = 8 + (index - 232) * 10;
    format!("#{gray:02x}{gray:02x}{gray:02x}")
}

fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('\"', "&quot;")
        .replace('\'', "&apos;")
}
