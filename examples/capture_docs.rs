//! Generates privacy-safe full-TUI product captures from the navigator renderer.
//!
//! The navigator pane is rendered by the real `NavigatorView::render` path. The
//! adjacent Codex pane uses fixed demonstration text, never a captured provider
//! screen, so the committed images cannot expose a user's prompts or results.
//! Run `scripts/capture-docs` from the repository root to refresh the assets.

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

const NAVIGATOR_WIDTH: u16 = 42;
const NAVIGATOR_HEIGHT: u16 = 18;
const PROVIDER_WIDTH: u16 = 73;
const CELL_WIDTH: u16 = 12;
const CELL_HEIGHT: u16 = 21;
const MARGIN: u16 = 18;
const HEADER_HEIGHT: u16 = 42;
const PANE_GUTTER: u16 = 24;
const TERMINAL_BACKGROUND: &str = "#101418";
const PROVIDER_BACKGROUND: &str = "#121820";

type CaptureResult<T> = Result<T, Box<dyn Error>>;

struct Capture<'a> {
    name: &'a str,
    step: &'a str,
    snapshot: LocalNavigatorSnapshot,
    provider: ProviderDemo<'a>,
}

struct ProviderDemo<'a> {
    prompt: &'a str,
    response: &'a [&'a str],
    footer: &'a str,
}

fn main() -> CaptureResult<()> {
    let output_root = PathBuf::from("docs/media/screenshots");
    fs::create_dir_all(&output_root)?;

    for capture in captures() {
        let mut terminal = Terminal::new(TestBackend::new(NAVIGATOR_WIDTH, NAVIGATOR_HEIGHT))?;
        let mut view = NavigatorView::new(capture.snapshot.clone());
        terminal.draw(|frame| view.render(frame))?;
        write_full_svg(
            &output_root.join(format!("{}.svg", capture.name)),
            terminal.backend().buffer(),
            &capture,
        )?;
    }

    Ok(())
}

fn captures() -> Vec<Capture<'static>> {
    vec![
        Capture {
            name: "open-workstream",
            step: "1 / 3  Open a Workstream",
            snapshot: open_workstream_snapshot(),
            provider: ProviderDemo {
                prompt: "Polish the navigator presentation",
                response: &[
                    "I’ll keep the navigator narrow and leave this native pane",
                    "directly interactive while I work through the presentation.",
                ],
                footer: "Ready · example workspace · Context 14% used",
            },
        },
        Capture {
            name: "fork-workstream",
            step: "2 / 3  Fork from the settled turn",
            snapshot: fork_workstream_snapshot(),
            provider: ProviderDemo {
                prompt: "Explore the alternate presentation in a new thread",
                response: &[
                    "This independent Workstream begins from the last completed",
                    "native turn. The source can keep running without interruption.",
                ],
                footer: "Ready · example workspace · Context 4% used",
            },
        },
        Capture {
            name: "park-and-resume",
            step: "3 / 3  Park safely, then resume natively",
            snapshot: park_and_resume_snapshot(),
            provider: ProviderDemo {
                prompt: "Resume the parked workstream",
                response: &[
                    "The exact native thread resumes with its visible result and",
                    "history intact. WSNav never reconstructs a transcript pane.",
                ],
                footer: "Ready · example workspace · Context 18% used",
            },
        },
    ]
}

fn open_workstream_snapshot() -> LocalNavigatorSnapshot {
    let now = now_millis();
    LocalNavigatorSnapshot {
        workstreams: vec![
            workstream(WorkstreamCapture {
                value: 101,
                project_id: project(1),
                host: NavigatorHost::Local,
                project_label: "workstream-navigator",
                display_name: "Polish the navigator presentation",
                runtime_status: NavigatorRuntimeStatus::Working,
                result_ready: false,
                last_activity_at_millis: now.saturating_sub(15_000),
            }),
            workstream(WorkstreamCapture {
                value: 102,
                project_id: project(2),
                host: remote_host("snap"),
                project_label: "release-notes",
                display_name: "Review the remote release notes",
                runtime_status: NavigatorRuntimeStatus::Attention,
                result_ready: true,
                last_activity_at_millis: now.saturating_sub(8 * 60_000),
            }),
            workstream(WorkstreamCapture {
                value: 103,
                project_id: project(3),
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

fn fork_workstream_snapshot() -> LocalNavigatorSnapshot {
    let now = now_millis();
    let navigator = project(1);
    LocalNavigatorSnapshot {
        workstreams: vec![
            workstream(WorkstreamCapture {
                value: 201,
                project_id: navigator,
                host: NavigatorHost::Local,
                project_label: "workstream-navigator",
                display_name: "Explore the alternate presentation",
                runtime_status: NavigatorRuntimeStatus::Idle,
                result_ready: false,
                last_activity_at_millis: now.saturating_sub(8_000),
            }),
            workstream(WorkstreamCapture {
                value: 202,
                project_id: navigator,
                host: NavigatorHost::Local,
                project_label: "workstream-navigator",
                display_name: "Polish the navigator presentation",
                runtime_status: NavigatorRuntimeStatus::Working,
                result_ready: false,
                last_activity_at_millis: now.saturating_sub(30_000),
            }),
            workstream(WorkstreamCapture {
                value: 203,
                project_id: project(2),
                host: remote_host("snap"),
                project_label: "release-notes",
                display_name: "Review the remote release notes",
                runtime_status: NavigatorRuntimeStatus::Attention,
                result_ready: true,
                last_activity_at_millis: now.saturating_sub(8 * 60_000),
            }),
        ],
        hosts: ready_hosts(),
        ..LocalNavigatorSnapshot::default()
    }
}

fn park_and_resume_snapshot() -> LocalNavigatorSnapshot {
    let now = now_millis();
    let navigator = project(1);
    LocalNavigatorSnapshot {
        workstreams: vec![
            workstream(WorkstreamCapture {
                value: 301,
                project_id: navigator,
                host: NavigatorHost::Local,
                project_label: "workstream-navigator",
                display_name: "Polish the navigator presentation",
                runtime_status: NavigatorRuntimeStatus::Idle,
                result_ready: true,
                last_activity_at_millis: now.saturating_sub(10_000),
            }),
            workstream(WorkstreamCapture {
                value: 302,
                project_id: navigator,
                host: NavigatorHost::Local,
                project_label: "workstream-navigator",
                display_name: "Explore the alternate presentation",
                runtime_status: NavigatorRuntimeStatus::Working,
                result_ready: false,
                last_activity_at_millis: now.saturating_sub(3 * 60_000),
            }),
            workstream(WorkstreamCapture {
                value: 303,
                project_id: project(2),
                host: remote_host("snap"),
                project_label: "release-notes",
                display_name: "Review the remote release notes",
                runtime_status: NavigatorRuntimeStatus::Parked,
                result_ready: false,
                last_activity_at_millis: now.saturating_sub(15 * 60_000),
            }),
        ],
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

fn write_full_svg(path: &Path, buffer: &Buffer, capture: &Capture<'_>) -> CaptureResult<()> {
    let navigator_width = NAVIGATOR_WIDTH * CELL_WIDTH;
    let provider_width = PROVIDER_WIDTH * CELL_WIDTH;
    let content_height = NAVIGATOR_HEIGHT * CELL_HEIGHT;
    let navigator_x = MARGIN;
    let navigator_y = MARGIN + HEADER_HEIGHT;
    let provider_x = navigator_x + navigator_width + PANE_GUTTER;
    let canvas_width = provider_x + provider_width + MARGIN;
    let canvas_height = navigator_y + content_height + MARGIN;
    let mut svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{canvas_width}\" height=\"{canvas_height}\" viewBox=\"0 0 {canvas_width} {canvas_height}\" role=\"img\" aria-label=\"Workstream Navigator two-pane tour\">\n<rect width=\"100%\" height=\"100%\" rx=\"14\" fill=\"{TERMINAL_BACKGROUND}\"/>\n"
    );

    write_text(
        &mut svg,
        navigator_x,
        29,
        "Workstream Navigator",
        "#e5e7eb",
        18,
        true,
    )?;
    write_text(&mut svg, provider_x, 29, capture.step, "#60a5fa", 16, false)?;
    writeln!(
        svg,
        "<rect x=\"{provider_x}\" y=\"{navigator_y}\" width=\"{provider_width}\" height=\"{content_height}\" fill=\"{PROVIDER_BACKGROUND}\"/>"
    )?;
    let divider_x = provider_x - PANE_GUTTER / 2;
    writeln!(
        svg,
        "<line x1=\"{divider_x}\" y1=\"{navigator_y}\" x2=\"{divider_x}\" y2=\"{}\" stroke=\"#334155\" stroke-width=\"2\"/>",
        navigator_y + content_height
    )?;

    write_buffer(&mut svg, buffer, navigator_x, navigator_y)?;
    write_provider_demo(
        &mut svg,
        provider_x,
        navigator_y,
        provider_width,
        &capture.provider,
    )?;
    svg.push_str("</svg>\n");
    fs::write(path, svg)?;
    Ok(())
}

fn write_buffer(
    svg: &mut String,
    buffer: &Buffer,
    origin_x: u16,
    origin_y: u16,
) -> CaptureResult<()> {
    for y in 0..NAVIGATOR_HEIGHT {
        for x in 0..NAVIGATOR_WIDTH {
            let cell = &buffer[(x, y)];
            let pixel_x = origin_x + x * CELL_WIDTH;
            let pixel_y = origin_y + y * CELL_HEIGHT;
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
            let weight = cell.modifier.contains(Modifier::BOLD);
            write_text(svg, pixel_x, pixel_y + 16, symbol, &color, 17, weight)?;
        }
    }
    Ok(())
}

fn write_provider_demo(
    svg: &mut String,
    provider_x: u16,
    provider_y: u16,
    provider_width: u16,
    provider: &ProviderDemo<'_>,
) -> CaptureResult<()> {
    let text_x = provider_x + 28;
    write_text(
        svg,
        text_x,
        provider_y + 35,
        "OpenAI Codex · privacy-safe demonstration",
        "#f8fafc",
        18,
        true,
    )?;
    write_text(
        svg,
        text_x,
        provider_y + 68,
        "model: gpt-5.6-sol    directory: example workspace",
        "#aab5c4",
        15,
        false,
    )?;
    writeln!(
        svg,
        "<line x1=\"{text_x}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"#334155\"/>",
        provider_y + 88,
        provider_x + provider_width - 28,
        provider_y + 88
    )?;
    write_text(svg, text_x, provider_y + 126, "›", "#60a5fa", 22, true)?;
    write_text(
        svg,
        text_x + 24,
        provider_y + 126,
        provider.prompt,
        "#f8fafc",
        17,
        false,
    )?;
    let mut response_y = provider_y + 180;
    for &line in provider.response {
        write_text(svg, text_x, response_y, line, "#cbd5e1", 17, false)?;
        response_y += 28;
    }
    let footer_y = provider_y + NAVIGATOR_HEIGHT * CELL_HEIGHT - 42;
    writeln!(
        svg,
        "<rect x=\"{provider_x}\" y=\"{}\" width=\"{provider_width}\" height=\"42\" fill=\"#0f172a\"/>",
        footer_y - 28
    )?;
    write_text(svg, text_x, footer_y, provider.footer, "#94a3b8", 15, false)?;
    Ok(())
}

fn write_text(
    svg: &mut String,
    x: u16,
    y: u16,
    text: &str,
    color: &str,
    font_size: u16,
    bold: bool,
) -> CaptureResult<()> {
    let weight = if bold { " font-weight=\"700\"" } else { "" };
    writeln!(
        svg,
        "<text x=\"{x}\" y=\"{y}\" fill=\"{color}\" font-family=\"JetBrains Mono, Iosevka, ui-monospace, monospace\" font-size=\"{font_size}\"{weight}>{}</text>",
        escape_xml(text)
    )?;
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
