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

/// The normal presentation is a 141-column terminal with the navigator held
/// to its deliberate 32-column tmux pane.  Keep the documentation captures at
/// that shape so they demonstrate the usable product layout rather than a
/// cropped illustration of the navigator alone.
const TERMINAL_WIDTH: u16 = 141;
const TERMINAL_HEIGHT: u16 = 60;
const NAVIGATOR_WIDTH: u16 = 32;
const PANE_DIVIDER_WIDTH: u16 = 1;
const PROVIDER_WIDTH: u16 = TERMINAL_WIDTH - NAVIGATOR_WIDTH - PANE_DIVIDER_WIDTH;
const CELL_WIDTH: u16 = 12;
const CELL_HEIGHT: u16 = 21;
const TERMINAL_BACKGROUND: &str = "#101418";
const PROVIDER_BACKGROUND: &str = "#0b0f14";

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
    activity: &'a [&'a str],
    footer: &'a str,
}

fn main() -> CaptureResult<()> {
    let output_root = PathBuf::from("docs/media/screenshots");
    fs::create_dir_all(&output_root)?;

    for capture in captures() {
        let mut terminal = Terminal::new(TestBackend::new(NAVIGATOR_WIDTH, TERMINAL_HEIGHT))?;
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
                activity: &[
                    "• Ran wsnav status",
                    "  Navigator layout is ready for the next native action.",
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
                activity: &[
                    "• Created the alternate native thread from the settled turn",
                    "  The original Workstream remains independently active.",
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
                activity: &[
                    "• Resumed the parked native thread in this provider pane",
                    "  The completed result remains visible until the next action.",
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
    let content_height = TERMINAL_HEIGHT * CELL_HEIGHT;
    let divider_width = PANE_DIVIDER_WIDTH * CELL_WIDTH;
    let navigator_x = 0;
    let navigator_y = 0;
    let divider_x = navigator_width;
    let provider_x = navigator_width + divider_width;
    let canvas_width = TERMINAL_WIDTH * CELL_WIDTH;
    let canvas_height = content_height;
    let mut svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{canvas_width}\" height=\"{canvas_height}\" viewBox=\"0 0 {canvas_width} {canvas_height}\" role=\"img\" aria-label=\"Workstream Navigator in a 141 by 60 terminal\">\n<rect width=\"100%\" height=\"100%\" fill=\"{TERMINAL_BACKGROUND}\"/>\n"
    );

    writeln!(
        svg,
        "<rect x=\"{provider_x}\" y=\"{navigator_y}\" width=\"{provider_width}\" height=\"{content_height}\" fill=\"{PROVIDER_BACKGROUND}\"/>"
    )?;
    writeln!(
        svg,
        "<rect x=\"{divider_x}\" y=\"0\" width=\"{divider_width}\" height=\"{content_height}\" fill=\"#1b222c\"/>\n<line x1=\"{}\" y1=\"0\" x2=\"{}\" y2=\"{content_height}\" stroke=\"#475569\" stroke-width=\"1\"/>",
        divider_x + divider_width / 2,
        divider_x + divider_width / 2,
    )?;
    let step_x = provider_x + provider_width - 35 * CELL_WIDTH;
    write_text(
        &mut svg,
        step_x,
        CELL_HEIGHT - 5,
        capture.step,
        "#60a5fa",
        14,
        false,
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
    for y in 0..TERMINAL_HEIGHT {
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
    let text_x = provider_x + 2 * CELL_WIDTH;
    let row_y = |row: u16| provider_y + row * CELL_HEIGHT + 16;
    write_provider_header(svg, text_x, provider_x, provider_y, provider_width)?;
    write_text(svg, text_x, row_y(10), "›", "#60a5fa", 22, true)?;
    write_text(
        svg,
        text_x + 2 * CELL_WIDTH,
        row_y(10),
        provider.prompt,
        "#f8fafc",
        17,
        false,
    )?;
    let mut response_row = 13;
    for &line in provider.response {
        write_text(svg, text_x, row_y(response_row), line, "#cbd5e1", 17, false)?;
        response_row += 2;
    }
    response_row += 2;
    for &line in provider.activity {
        let color = if line.starts_with('•') {
            "#60a5fa"
        } else {
            "#aab5c4"
        };
        write_text(svg, text_x, row_y(response_row), line, color, 16, false)?;
        response_row += 2;
    }
    write_text(
        svg,
        text_x,
        row_y(45),
        "The native terminal remains the working surface.",
        "#64748b",
        15,
        false,
    )?;
    write_text(svg, text_x, row_y(52), "›", "#60a5fa", 22, true)?;
    write_text(
        svg,
        text_x + 2 * CELL_WIDTH,
        row_y(52),
        "Continue in the native Codex terminal…",
        "#64748b",
        17,
        false,
    )?;
    write_provider_footer(
        svg,
        text_x,
        provider_x,
        provider_y,
        provider_width,
        provider.footer,
    )
}

fn write_provider_header(
    svg: &mut String,
    text_x: u16,
    provider_x: u16,
    provider_y: u16,
    provider_width: u16,
) -> CaptureResult<()> {
    let row_y = |row: u16| provider_y + row * CELL_HEIGHT + 16;
    write_text(
        svg,
        text_x,
        row_y(1),
        "OpenAI Codex (demonstration)",
        "#f8fafc",
        18,
        true,
    )?;
    write_text(
        svg,
        text_x,
        row_y(3),
        "model:       gpt-5.6-sol xhigh    /model to change",
        "#aab5c4",
        15,
        false,
    )?;
    write_text(
        svg,
        text_x,
        row_y(4),
        "directory:   example workspace",
        "#aab5c4",
        15,
        false,
    )?;
    write_text(
        svg,
        text_x,
        row_y(5),
        "permissions: workspace-write",
        "#aab5c4",
        15,
        false,
    )?;
    writeln!(
        svg,
        "<line x1=\"{text_x}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"#334155\"/>",
        provider_y + 7 * CELL_HEIGHT,
        provider_x + provider_width - 2 * CELL_WIDTH,
        provider_y + 7 * CELL_HEIGHT,
    )?;
    Ok(())
}

fn write_provider_footer(
    svg: &mut String,
    text_x: u16,
    provider_x: u16,
    provider_y: u16,
    provider_width: u16,
    footer: &str,
) -> CaptureResult<()> {
    let row_y = |row: u16| provider_y + row * CELL_HEIGHT + 16;
    let footer_y = provider_y + (TERMINAL_HEIGHT - 2) * CELL_HEIGHT;
    writeln!(
        svg,
        "<rect x=\"{provider_x}\" y=\"{footer_y}\" width=\"{provider_width}\" height=\"{}\" fill=\"#111827\"/>",
        2 * CELL_HEIGHT,
    )?;
    write_text(svg, text_x, row_y(59), footer, "#94a3b8", 15, false)?;
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
