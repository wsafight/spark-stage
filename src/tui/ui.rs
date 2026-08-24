use std::path::Path;

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Cell, Clear, List, ListItem, ListState, Paragraph, Row, Table, TableState, Wrap,
};

use super::app::{App, ConnectionState, Page, StatusKind};
use super::backend::TuiBackend;
use super::protocol::{
    ApprovalSummary, BuildSummary, DiagnosticSummary, QueueJobSummary, ShotSummary, TakeSummary,
};

const WIDE_TERMINAL: u16 = 96;

const BORDER: Style = Style::new().fg(Color::DarkGray);
const ACTIVE: Style = Style::new()
    .fg(Color::Black)
    .bg(Color::Cyan)
    .add_modifier(Modifier::BOLD);
const LABEL: Style = Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD);
const MUTED: Style = Style::new().fg(Color::DarkGray);

pub fn render<B: TuiBackend>(frame: &mut Frame<'_>, app: &App<B>) {
    let area = frame.area();
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(4),
            Constraint::Length(2),
        ])
        .split(area);

    render_header(frame, layout[0], app);
    render_navigation(frame, layout[1], app);
    render_body(frame, layout[2], app);
    render_footer(frame, layout[3], app);

    if app.show_help {
        render_help(frame, area);
    } else if let Some(confirmation) = &app.confirmation {
        render_confirmation(frame, area, &confirmation.prompt);
    }
}

fn render_header<B: TuiBackend>(frame: &mut Frame<'_>, area: Rect, app: &App<B>) {
    let (connection, connection_style) = match &app.connection {
        ConnectionState::Connected => ("CONNECTED", Style::new().fg(Color::Green)),
        ConnectionState::Disconnected(_) => ("DISCONNECTED", Style::new().fg(Color::Red)),
    };
    let project = app.snapshot.as_ref().map_or_else(
        || "No project snapshot".to_owned(),
        |snapshot| {
            format!(
                "{} [{} / {}] rev {}",
                snapshot.project.title,
                snapshot.project.stage,
                snapshot.project.outcome,
                snapshot.revision
            )
        },
    );
    let line = Line::from(vec![
        Span::styled(" SparkStage ", ACTIVE),
        Span::raw(" "),
        Span::styled(format!("[{connection}]"), connection_style),
        Span::raw(format!("  {project}")),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

fn render_navigation<B: TuiBackend>(frame: &mut Frame<'_>, area: Rect, app: &App<B>) {
    let mut spans = Vec::new();
    if area.width >= 76 {
        for (index, page) in Page::ALL.into_iter().enumerate() {
            if index > 0 {
                spans.push(Span::raw("  "));
            }
            let text = format!("{} {}", index + 1, page.title());
            spans.push(Span::styled(
                text,
                if app.page == page { ACTIVE } else { MUTED },
            ));
        }
    } else {
        spans.push(Span::styled(
            format!("[{}] {}", app.page.index() + 1, app.page.title()),
            ACTIVE,
        ));
        spans.push(Span::styled("  Tab: next  ?: help", MUTED));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn render_body<B: TuiBackend>(frame: &mut Frame<'_>, area: Rect, app: &App<B>) {
    if app.snapshot.is_none() {
        let detail = match &app.connection {
            ConnectionState::Connected => "Worker returned no snapshot".to_owned(),
            ConnectionState::Disconnected(message) => message.clone(),
        };
        frame.render_widget(
            Paragraph::new(vec![
                Line::styled("Waiting for worker snapshot", LABEL),
                Line::raw(detail),
                Line::raw("Press g to retry. Press ? for help."),
            ])
            .block(panel("Worker unavailable"))
            .wrap(Wrap { trim: false }),
            area,
        );
        return;
    }

    match app.page {
        Page::Dashboard => render_dashboard(frame, area, app),
        Page::Shots => render_shots(frame, area, app),
        Page::Takes => render_takes(frame, area, app),
        Page::Queue => render_queue(frame, area, app),
        Page::Builds => render_builds(frame, area, app),
        Page::Diagnostics => render_diagnostics(frame, area, app),
    }
}

fn render_dashboard<B: TuiBackend>(frame: &mut Frame<'_>, area: Rect, app: &App<B>) {
    let snapshot = app.snapshot.as_ref().expect("checked by render_body");
    let items = snapshot
        .pending_approvals
        .iter()
        .map(|approval| {
            let blocking = if approval.blocking {
                "BLOCKING"
            } else {
                "optional"
            };
            ListItem::new(format!(
                "{} | {} | {}",
                approval.approval_id, approval.kind, blocking
            ))
        })
        .collect();
    let title = format!("Pending approvals ({})", snapshot.pending_approvals.len());
    let details = dashboard_details(app.selected_approval(), snapshot);
    render_master_detail(
        frame,
        area,
        &title,
        items,
        app.selection(),
        "Project status",
        details,
    );
}

fn dashboard_details(
    approval: Option<&ApprovalSummary>,
    snapshot: &super::protocol::AppSnapshot,
) -> Vec<Line<'static>> {
    let mut lines = vec![
        section("PROJECT"),
        field("ID", &snapshot.project.id),
        field("Stage", &snapshot.project.stage),
        field("Outcome", &snapshot.project.outcome),
        field("Mode", &snapshot.project.work_mode),
        field("Target", &snapshot.project.quality_target),
        Line::raw(""),
        section("GPU"),
        field("Status", &snapshot.gpu.status),
        field("Job", option_text(snapshot.gpu.job_id.as_deref())),
        field("Shot", option_text(snapshot.gpu.shot_id.as_deref())),
        field("Progress", &format_progress(snapshot.gpu.progress)),
        field(
            "ETA",
            &format_seconds(snapshot.gpu.eta_seconds.unwrap_or(0)),
        ),
        Line::raw(""),
        section("BUDGET"),
        field("Elapsed", &format_seconds(snapshot.budget.elapsed_seconds)),
        field(
            "Remaining",
            &format_seconds(snapshot.budget.estimated_remaining_seconds),
        ),
        field(
            "Disk",
            &format!(
                "{} free / {} required",
                format_bytes(snapshot.budget.disk_free_bytes),
                format_bytes(snapshot.budget.disk_required_bytes)
            ),
        ),
        field(
            "Auditions",
            &format!(
                "{} / {}",
                snapshot.budget.audition_takes_used, snapshot.budget.audition_takes_limit
            ),
        ),
        Line::raw(""),
        section("SELECTED APPROVAL"),
    ];
    if let Some(approval) = approval {
        lines.extend([
            field("ID", &approval.approval_id),
            field("Kind", &approval.kind),
            field("Shot", option_text(approval.shot_id.as_deref())),
            field("Takes", &list_text(&approval.take_ids)),
            field("Blocking", yes_no(approval.blocking)),
            field("Reason", &approval.description),
        ]);
    } else {
        lines.push(Line::styled("No pending approval", MUTED));
    }
    lines.push(Line::raw(""));
    lines.push(section("RECENT FAILURES"));
    if snapshot.recent_failures.is_empty() {
        lines.push(Line::styled("None", MUTED));
    } else {
        for failure in &snapshot.recent_failures {
            lines.push(Line::styled(
                format!("[{}] {}", failure.code, failure.subject),
                Style::new().fg(Color::Red),
            ));
            lines.push(Line::raw(format!(
                "{} | {}",
                failure.occurred_at, failure.message
            )));
        }
    }
    lines
}

fn render_shots<B: TuiBackend>(frame: &mut Frame<'_>, area: Rect, app: &App<B>) {
    let snapshot = app.snapshot.as_ref().expect("checked by render_body");
    let rows = snapshot
        .shots
        .iter()
        .map(|shot| {
            Row::new(vec![
                Cell::from(shot.shot_id.clone()),
                Cell::from(shot.title.clone()),
                Cell::from(shot.stage.clone()),
                Cell::from(shot.risk.clone()),
                Cell::from(shot.candidate_count.to_string()),
                Cell::from(shot_flags(shot)),
            ])
        })
        .collect();
    let details = shot_details(app.selected_shot());
    render_table_detail(
        frame,
        area,
        &format!("Shots ({})", snapshot.shots.len()),
        ["ID", "Title", "Stage", "Risk", "Takes", "Flags"],
        rows,
        [
            Constraint::Length(10),
            Constraint::Min(12),
            Constraint::Length(18),
            Constraint::Length(8),
            Constraint::Length(7),
            Constraint::Length(16),
        ],
        app.selection(),
        "Shot details",
        details,
    );
}

fn shot_details(shot: Option<&ShotSummary>) -> Vec<Line<'static>> {
    let Some(shot) = shot else {
        return vec![Line::styled("No shot selected", MUTED)];
    };
    vec![
        field("ID", &shot.shot_id),
        field("Title", &shot.title),
        field("Stage", &shot.stage),
        field("Risk", &shot.risk),
        field("Candidates", &shot.candidate_count.to_string()),
        field(
            "Selected take",
            option_text(shot.selected_take_id.as_deref()),
        ),
        field(
            "Approved take",
            option_text(shot.approved_take_id.as_deref()),
        ),
        field("Stale", yes_no(shot.stale)),
        field("Failure codes", &list_text(&shot.fail_codes)),
    ]
}

fn render_takes<B: TuiBackend>(frame: &mut Frame<'_>, area: Rect, app: &App<B>) {
    let takes = app.visible_takes();
    let rows = takes
        .iter()
        .map(|take| {
            Row::new(vec![
                Cell::from(take.take_id.clone()),
                Cell::from(take.profile.clone()),
                Cell::from(take.status.clone()),
                Cell::from(format_score(take.score)),
                Cell::from(take_flags(take)),
            ])
        })
        .collect();
    let filter = app.selected_shot_id.as_deref().unwrap_or("all shots");
    render_table_detail(
        frame,
        area,
        &format!("Takes for {filter} ({})", takes.len()),
        ["Take ID", "Profile", "Status", "Score", "Flags"],
        rows,
        [
            Constraint::Length(18),
            Constraint::Length(12),
            Constraint::Min(14),
            Constraint::Length(8),
            Constraint::Length(18),
        ],
        app.selection(),
        "Take details",
        take_details(app.selected_take()),
    );
}

fn take_details(take: Option<&TakeSummary>) -> Vec<Line<'static>> {
    let Some(take) = take else {
        return vec![Line::styled("No take selected for this shot", MUTED)];
    };
    vec![
        field("Take ID", &take.take_id),
        field("Shot ID", &take.shot_id),
        field("Profile", &take.profile),
        field("Status", &take.status),
        field("Score", &format_score(take.score)),
        field("Selected", yes_no(take.selected)),
        field("Approved", yes_no(take.approved)),
        field("Hard checks", &list_text(&take.hard_checks)),
        field("Warnings", &list_text(&take.warnings)),
        field("Media", path_text(take.media_path.as_deref())),
    ]
}

fn render_queue<B: TuiBackend>(frame: &mut Frame<'_>, area: Rect, app: &App<B>) {
    let queue = &app.snapshot.as_ref().expect("checked by render_body").queue;
    let rows = queue
        .jobs
        .iter()
        .map(|job| {
            Row::new(vec![
                Cell::from(job.job_id.clone()),
                Cell::from(job.subject.clone()),
                Cell::from(job.state.clone()),
                Cell::from(job.priority.clone()),
                Cell::from(job.resource.clone()),
                Cell::from(format_progress(job.progress)),
            ])
        })
        .collect();
    let queue_state = if queue.paused { "PAUSED" } else { "RUNNING" };
    render_table_detail(
        frame,
        area,
        &format!("Queue {queue_state} ({})", queue.jobs.len()),
        [
            "Job ID", "Subject", "State", "Priority", "Resource", "Progress",
        ],
        rows,
        [
            Constraint::Length(18),
            Constraint::Min(13),
            Constraint::Length(18),
            Constraint::Length(10),
            Constraint::Length(13),
            Constraint::Length(10),
        ],
        app.selection(),
        "Job details",
        queue_details(app.selected_queue_job(), queue.paused),
    );
}

fn queue_details(job: Option<&QueueJobSummary>, paused: bool) -> Vec<Line<'static>> {
    let mut lines = vec![field(
        "Queue state",
        if paused { "PAUSED" } else { "RUNNING" },
    )];
    let Some(job) = job else {
        lines.push(Line::styled("No queued job selected", MUTED));
        return lines;
    };
    lines.extend([
        field("Job ID", &job.job_id),
        field("Subject", &job.subject),
        field("State", &job.state),
        field("Priority", &job.priority),
        field("Resource", &job.resource),
        field("Progress", &format_progress(job.progress)),
        field("ETA", &format_seconds(job.eta_seconds.unwrap_or(0))),
    ]);
    lines
}

fn render_builds<B: TuiBackend>(frame: &mut Frame<'_>, area: Rect, app: &App<B>) {
    let builds = &app
        .snapshot
        .as_ref()
        .expect("checked by render_body")
        .builds;
    let rows = builds
        .iter()
        .map(|build| {
            Row::new(vec![
                Cell::from(build.build_id.clone()),
                Cell::from(build.kind.clone()),
                Cell::from(build.status.clone()),
                Cell::from(build.recipe.clone()),
                Cell::from(if build.warnings.is_empty() {
                    "none".to_owned()
                } else {
                    format!("{} warning(s)", build.warnings.len())
                }),
            ])
        })
        .collect();
    render_table_detail(
        frame,
        area,
        &format!("Builds ({})", builds.len()),
        ["Build ID", "Kind", "Status", "Recipe", "Warnings"],
        rows,
        [
            Constraint::Length(18),
            Constraint::Length(10),
            Constraint::Length(15),
            Constraint::Min(16),
            Constraint::Length(13),
        ],
        app.selection(),
        "Build details",
        build_details(app.selected_build()),
    );
}

fn build_details(build: Option<&BuildSummary>) -> Vec<Line<'static>> {
    let Some(build) = build else {
        return vec![
            Line::styled("No build selected", MUTED),
            Line::raw("Press b to create a draft build."),
        ];
    };
    vec![
        field("Build ID", &build.build_id),
        field("Kind", &build.kind),
        field("Status", &build.status),
        field("Recipe", &build.recipe),
        field("Output", path_text(build.output_path.as_deref())),
        field("Warnings", &list_text(&build.warnings)),
    ]
}

fn render_diagnostics<B: TuiBackend>(frame: &mut Frame<'_>, area: Rect, app: &App<B>) {
    let diagnostics = &app
        .snapshot
        .as_ref()
        .expect("checked by render_body")
        .diagnostics;
    let rows = diagnostics
        .iter()
        .map(|probe| {
            Row::new(vec![
                Cell::from(probe.probe_id.clone()),
                Cell::from(probe.component.clone()),
                Cell::from(probe.status.clone()),
                Cell::from(probe.summary.clone()),
            ])
        })
        .collect();
    render_table_detail(
        frame,
        area,
        &format!("Diagnostics ({})", diagnostics.len()),
        ["Probe ID", "Component", "Status", "Summary"],
        rows,
        [
            Constraint::Length(18),
            Constraint::Length(18),
            Constraint::Length(14),
            Constraint::Min(20),
        ],
        app.selection(),
        "Probe details",
        diagnostic_details(app.selected_diagnostic()),
    );
}

fn diagnostic_details(probe: Option<&DiagnosticSummary>) -> Vec<Line<'static>> {
    let Some(probe) = probe else {
        return vec![Line::styled("No diagnostic selected", MUTED)];
    };
    vec![
        field("Probe ID", &probe.probe_id),
        field("Component", &probe.component),
        field("Status", &probe.status),
        field("Summary", &probe.summary),
        field("Capabilities", &list_text(&probe.capabilities)),
    ]
}

fn render_master_detail(
    frame: &mut Frame<'_>,
    area: Rect,
    list_title: &str,
    items: Vec<ListItem<'static>>,
    selected: usize,
    detail_title: &str,
    detail_lines: Vec<Line<'static>>,
) {
    let [master, detail] = master_detail_areas(area);
    let has_items = !items.is_empty();
    let list = List::new(items)
        .block(panel(list_title))
        .highlight_style(ACTIVE)
        .highlight_symbol("> ");
    let mut state = ListState::default().with_selected(has_items.then_some(selected));
    frame.render_stateful_widget(list, master, &mut state);
    render_details(frame, detail, detail_title, detail_lines);
}

#[allow(clippy::too_many_arguments)]
fn render_table_detail<const COLUMNS: usize>(
    frame: &mut Frame<'_>,
    area: Rect,
    table_title: &str,
    headers: [&str; COLUMNS],
    rows: Vec<Row<'static>>,
    widths: [Constraint; COLUMNS],
    selected: usize,
    detail_title: &str,
    detail_lines: Vec<Line<'static>>,
) {
    let [master, detail] = master_detail_areas(area);
    let has_rows = !rows.is_empty();
    let header = Row::new(
        headers
            .into_iter()
            .map(|header| Cell::from(header.to_owned())),
    )
    .style(LABEL)
    .bottom_margin(1);
    let table = Table::new(rows, widths)
        .header(header)
        .block(panel(table_title))
        .column_spacing(1)
        .row_highlight_style(ACTIVE)
        .highlight_symbol("> ");
    let mut state = TableState::default().with_selected(has_rows.then_some(selected));
    frame.render_stateful_widget(table, master, &mut state);
    render_details(frame, detail, detail_title, detail_lines);
}

fn master_detail_areas(area: Rect) -> [Rect; 2] {
    if area.width >= WIDE_TERMINAL {
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(62), Constraint::Percentage(38)])
            .split(area);
        [chunks[0], chunks[1]]
    } else {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(48), Constraint::Percentage(52)])
            .split(area);
        [chunks[0], chunks[1]]
    }
}

fn render_details(frame: &mut Frame<'_>, area: Rect, title: &str, lines: Vec<Line<'static>>) {
    frame.render_widget(
        Paragraph::new(lines)
            .block(panel(title))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn render_footer<B: TuiBackend>(frame: &mut Frame<'_>, area: Rect, app: &App<B>) {
    let actions = match app.page {
        Page::Dashboard => "a approve | Enter open shot",
        Page::Shots => "u audition | d direct render | r retry | Enter takes",
        Page::Takes => "s select | a approve | x reject | p/Enter preview",
        Page::Queue => "Space pause/resume | x cancel",
        Page::Builds => "b build/rebuild | o/Enter open",
        Page::Diagnostics => "r retry probe | l open logs",
    };
    let status_style = match app.status.kind {
        StatusKind::Info => Style::new().fg(Color::Cyan),
        StatusKind::Success => Style::new().fg(Color::Green),
        StatusKind::Warning => Style::new().fg(Color::Yellow),
        StatusKind::Error => Style::new().fg(Color::Red),
    };
    let status_label = match app.status.kind {
        StatusKind::Info => "INFO",
        StatusKind::Success => "OK",
        StatusKind::Warning => "WARN",
        StatusKind::Error => "ERROR",
    };
    let paragraph = Paragraph::new(vec![
        Line::from(vec![
            Span::styled(format!("[{status_label}] "), status_style),
            Span::raw(&app.status.text),
        ]),
        Line::styled(format!("{actions} | g refresh | q quit | ? help"), MUTED),
    ]);
    frame.render_widget(paragraph, area);
}

fn render_help(frame: &mut Frame<'_>, area: Rect) {
    let popup = centered_rect(area, 74, 24);
    frame.render_widget(Clear, popup);
    let help = Paragraph::new(vec![
        section("GLOBAL"),
        Line::raw("1-6 page | Tab/Shift-Tab or Left/Right page"),
        Line::raw("j/k or Down/Up select | Enter open | g refresh"),
        Line::raw("? or Esc close help | q quit"),
        Line::raw(""),
        section("ACTIONS"),
        Line::raw("Dashboard: a approve"),
        Line::raw("Shots: u audition, d direct render, r retry"),
        Line::raw("Takes: s select, a approve, x reject, p preview"),
        Line::raw("Queue: Space pause/resume, x cancel"),
        Line::raw("Builds: b build/rebuild, o open"),
        Line::raw("Diagnostics: r retry probe, l open logs"),
        Line::raw(""),
        Line::styled(
            "Mutations require y/Enter confirmation. n/Esc cancels.",
            Style::new().fg(Color::Yellow),
        ),
    ])
    .alignment(Alignment::Left)
    .block(panel("Help"))
    .wrap(Wrap { trim: false });
    frame.render_widget(help, popup);
}

fn render_confirmation(frame: &mut Frame<'_>, area: Rect, prompt: &str) {
    let popup = centered_rect(area, 68, 8);
    frame.render_widget(Clear, popup);
    let paragraph = Paragraph::new(vec![
        Line::raw(prompt.to_owned()),
        Line::raw(""),
        Line::from(vec![
            Span::styled("y / Enter", Style::new().fg(Color::Green)),
            Span::raw(" confirm    "),
            Span::styled("n / Esc", Style::new().fg(Color::Yellow)),
            Span::raw(" cancel"),
        ]),
    ])
    .block(panel("Confirm action"))
    .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, popup);
}

fn centered_rect(area: Rect, max_width: u16, max_height: u16) -> Rect {
    let width = max_width.min(area.width.saturating_sub(2)).max(1);
    let height = max_height.min(area.height.saturating_sub(2)).max(1);
    Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    }
}

fn panel<'a>(title: impl Into<Line<'a>>) -> Block<'a> {
    Block::default()
        .borders(Borders::ALL)
        .border_style(BORDER)
        .title(title)
}

fn section(value: &str) -> Line<'static> {
    Line::styled(value.to_owned(), LABEL)
}

fn field(label: &str, value: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{label}: "), LABEL),
        Span::raw(value.to_owned()),
    ])
}

fn list_text(values: &[String]) -> String {
    if values.is_empty() {
        "none".to_owned()
    } else {
        values.join(", ")
    }
}

fn option_text(value: Option<&str>) -> &str {
    value.unwrap_or("none")
}

fn path_text(value: Option<&Path>) -> &str {
    value.and_then(Path::to_str).unwrap_or("none")
}

fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

fn shot_flags(shot: &ShotSummary) -> String {
    let mut flags = Vec::new();
    if shot.stale {
        flags.push("STALE".to_owned());
    }
    if !shot.fail_codes.is_empty() {
        flags.push(format!("FAIL:{}", shot.fail_codes.join(",")));
    }
    if shot.approved_take_id.is_some() {
        flags.push("APPROVED".to_owned());
    } else if shot.selected_take_id.is_some() {
        flags.push("SELECTED".to_owned());
    }
    if flags.is_empty() {
        "none".to_owned()
    } else {
        flags.join(" ")
    }
}

fn take_flags(take: &TakeSummary) -> String {
    let mut flags = Vec::new();
    if take.selected {
        flags.push("SELECTED");
    }
    if take.approved {
        flags.push("APPROVED");
    }
    if !take.warnings.is_empty() {
        flags.push("WARN");
    }
    if flags.is_empty() {
        "none".to_owned()
    } else {
        flags.join(" ")
    }
}

fn format_score(value: Option<f64>) -> String {
    value.map_or_else(|| "n/a".to_owned(), |score| format!("{score:.2}"))
}

fn format_progress(value: Option<f64>) -> String {
    value.map_or_else(
        || "n/a".to_owned(),
        |progress| {
            let percent = if progress <= 1.0 {
                progress * 100.0
            } else {
                progress
            };
            format!("{percent:.0}%")
        },
    )
}

fn format_seconds(seconds: u64) -> String {
    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    let seconds = seconds % 60;
    if hours > 0 {
        format!("{hours}h {minutes:02}m {seconds:02}s")
    } else if minutes > 0 {
        format!("{minutes}m {seconds:02}s")
    } else {
        format!("{seconds}s")
    }
}

fn format_bytes(bytes: u64) -> String {
    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
    const MIB: f64 = 1024.0 * 1024.0;
    let bytes = bytes as f64;
    if bytes >= GIB {
        format!("{:.1} GiB", bytes / GIB)
    } else if bytes >= MIB {
        format!("{:.1} MiB", bytes / MIB)
    } else {
        format!("{bytes:.0} B")
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;
    use crate::tui::backend::{BackendError, BackendReply};
    use crate::tui::protocol::{
        AppSnapshot, BudgetSummary, GpuSummary, ProjectSummary, QueueSummary,
    };
    use crate::tui::protocol::{FailureSummary, WorkerCommand};

    #[derive(Clone)]
    struct RenderBackend {
        snapshot: AppSnapshot,
    }

    impl TuiBackend for RenderBackend {
        fn refresh(&mut self) -> Result<AppSnapshot, BackendError> {
            Ok(self.snapshot.clone())
        }

        fn dispatch(
            &mut self,
            _command: WorkerCommand,
            _expected_revision: u64,
        ) -> Result<BackendReply, BackendError> {
            Ok(BackendReply {
                snapshot: Some(self.snapshot.clone()),
                artifact_path: None,
                message: Some("accepted".to_owned()),
            })
        }
    }

    fn snapshot() -> AppSnapshot {
        AppSnapshot {
            schema_version: "1.0".to_owned(),
            revision: 42,
            refreshed_at: "2026-08-25T12:00:00Z".to_owned(),
            project: ProjectSummary {
                id: "rain-apartment".to_owned(),
                title: "Rain Apartment".to_owned(),
                stage: "shooting".to_owned(),
                outcome: "needs_review".to_owned(),
                work_mode: "director".to_owned(),
                quality_target: "playable".to_owned(),
            },
            gpu: GpuSummary {
                status: "busy".to_owned(),
                job_id: Some("JOB-VERY-LONG-0001".to_owned()),
                shot_id: Some("S01".to_owned()),
                progress: Some(0.43),
                eta_seconds: Some(137),
            },
            budget: BudgetSummary {
                elapsed_seconds: 340,
                estimated_remaining_seconds: 800,
                disk_free_bytes: 128 * 1024 * 1024 * 1024,
                disk_required_bytes: 32 * 1024 * 1024 * 1024,
                audition_takes_used: 2,
                audition_takes_limit: 6,
            },
            pending_approvals: vec![ApprovalSummary {
                approval_id: "APPROVAL-LONG-ID-0001".to_owned(),
                kind: "candidate_selection".to_owned(),
                shot_id: Some("S01".to_owned()),
                take_ids: vec!["S01-T001".to_owned(), "S01-T002".to_owned()],
                blocking: true,
                description: "Choose the stable identity take".to_owned(),
            }],
            recent_failures: vec![FailureSummary {
                code: "BLACK_FRAME_AT_BOUNDARY".to_owned(),
                subject: "S01-T001".to_owned(),
                message: "Last frame failed the luma threshold".to_owned(),
                occurred_at: "12:03:04Z".to_owned(),
            }],
            shots: vec![ShotSummary {
                shot_id: "S01".to_owned(),
                title: "Door in the rain".to_owned(),
                stage: "candidates_ready".to_owned(),
                risk: "high".to_owned(),
                candidate_count: 2,
                selected_take_id: Some("S01-T002".to_owned()),
                approved_take_id: None,
                fail_codes: vec!["BLACK_FRAME_AT_BOUNDARY".to_owned()],
                stale: true,
            }],
            takes: vec![TakeSummary {
                take_id: "S01-T002".to_owned(),
                shot_id: "S01".to_owned(),
                profile: "audition".to_owned(),
                status: "validated".to_owned(),
                score: Some(0.82),
                hard_checks: vec!["duration_ok".to_owned()],
                warnings: vec!["LIP_SYNC_REVIEW".to_owned()],
                selected: true,
                approved: false,
                media_path: Some("/projects/rain/review/S01/S01-T002-preview.mp4".into()),
            }],
            queue: QueueSummary {
                paused: false,
                jobs: vec![],
            },
            builds: vec![],
            diagnostics: vec![],
        }
    }

    fn app() -> App<RenderBackend> {
        let backend = RenderBackend {
            snapshot: snapshot(),
        };
        let mut app = App::new(backend, Duration::from_secs(2));
        app.initial_refresh();
        app
    }

    fn draw(app: &App<RenderBackend>, width: u16, height: u16) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| render(frame, app)).unwrap();
        let buffer = terminal.backend().buffer();
        let mut output = String::new();
        for y in 0..height {
            for x in 0..width {
                output.push_str(buffer.cell((x, y)).unwrap().symbol());
            }
            output.push('\n');
        }
        output
    }

    #[test]
    fn wide_dashboard_has_operational_context() {
        let output = draw(&app(), 120, 38);
        assert!(output.contains("SparkStage"));
        assert!(output.contains("CONNECTED"));
        assert!(output.contains("Pending approvals"));
        assert!(output.contains("APPROVAL-LONG-ID-0001"));
        assert!(output.contains("BLACK_FRAME_AT_BOUNDARY"));
    }

    #[test]
    fn narrow_shot_layout_keeps_full_failure_code_in_details() {
        let mut app = app();
        app.page = Page::Shots;
        let output = draw(&app, 58, 36);
        assert!(output.contains("Shot details"));
        assert!(output.contains("BLACK_FRAME_AT_BOUNDARY"));
        assert!(output.contains("S01-T002"));
    }

    #[test]
    fn confirmation_overlay_names_the_mutation() {
        let mut app = app();
        app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
        let output = draw(&app, 100, 30);
        assert!(output.contains("Confirm action"));
        assert!(output.contains("candidate_selection"));
        assert!(output.contains("y / Enter"));
    }
}
