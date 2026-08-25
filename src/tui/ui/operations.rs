use super::*;

pub(super) fn render_projects<B: TuiBackend>(frame: &mut Frame<'_>, area: Rect, app: &App<B>) {
    let rows = app
        .projects
        .iter()
        .map(|project| {
            Row::new(vec![
                Cell::from(project.id.clone()),
                Cell::from(project.title.as_deref().unwrap_or("unavailable").to_owned()),
                Cell::from(project.stage.as_deref().unwrap_or("unknown").to_owned()),
                Cell::from(project.outcome.as_deref().unwrap_or("unknown").to_owned()),
                Cell::from(if project.paused { "PAUSED" } else { "active" }),
                Cell::from(
                    project
                        .revision
                        .map_or_else(|| "n/a".to_owned(), |value| value.to_string()),
                ),
            ])
        })
        .collect();
    render_table_detail(
        frame,
        area,
        &format!("Projects ({})", app.projects.len()),
        ["ID", "Title", "Stage", "Outcome", "State", "Revision"],
        rows,
        [
            Constraint::Length(20),
            Constraint::Min(16),
            Constraint::Length(14),
            Constraint::Length(16),
            Constraint::Length(9),
            Constraint::Length(9),
        ],
        app.selection(),
        "Project details",
        project_details(app.selected_project()),
    );
}

fn project_details(project: Option<&ProjectListItem>) -> Vec<Line<'static>> {
    let Some(project) = project else {
        return vec![Line::styled("No project available", MUTED)];
    };
    vec![
        field("ID", &project.id),
        field("Title", project.title.as_deref().unwrap_or("unavailable")),
        field("Stage", project.stage.as_deref().unwrap_or("unknown")),
        field("Outcome", project.outcome.as_deref().unwrap_or("unknown")),
        field("Paused", yes_no(project.paused)),
        field(
            "Revision",
            &project
                .revision
                .map_or_else(|| "n/a".to_owned(), |value| value.to_string()),
        ),
        field(
            "Updated",
            project.updated_at.as_deref().unwrap_or("unknown"),
        ),
        field("Error", project.error.as_deref().unwrap_or("none")),
    ]
}

pub(super) fn render_review<B: TuiBackend>(frame: &mut Frame<'_>, area: Rect, app: &App<B>) {
    let rows = app
        .review_rows()
        .into_iter()
        .map(|review| {
            Row::new(vec![
                Cell::from(if review.included { "yes" } else { "no" }),
                Cell::from(review.shot_id),
                Cell::from(review.take_id),
                Cell::from(review.take_ids.len().to_string()),
                Cell::from(review.warning_count.to_string()),
            ])
        })
        .collect::<Vec<_>>();
    let details = app.selected_review_row().map_or_else(
        || vec![Line::styled("No candidate approval pending", MUTED)],
        |review| {
            vec![
                field("Shot", &review.shot_id),
                field("Chosen take", &review.take_id),
                field("Included", yes_no(review.included)),
                field("Warnings", &review.warning_count.to_string()),
                field("Candidates", &list_text(&review.take_ids)),
            ]
        },
    );
    render_table_detail(
        frame,
        area,
        "Batch review",
        ["Include", "Shot", "Chosen take", "Takes", "Warnings"],
        rows,
        [
            Constraint::Length(9),
            Constraint::Length(10),
            Constraint::Min(20),
            Constraint::Length(7),
            Constraint::Length(10),
        ],
        app.selection(),
        "Review choice",
        details,
    );
}

pub(super) fn render_storage<B: TuiBackend>(frame: &mut Frame<'_>, area: Rect, app: &App<B>) {
    let mut lines = vec![section("STORAGE")];
    if let Some(report) = &app.storage_report {
        lines.extend([
            field("Project", &report.project_id),
            field("Total", &format_bytes(report.total_bytes)),
            field("Trash", &format_bytes(report.trash_bytes)),
            field("Reclaimable", &format_bytes(report.reclaimable_bytes)),
            field("Files", &report.reclaimable_files.to_string()),
        ]);
    } else {
        lines.push(Line::styled("Storage report not loaded", MUTED));
    }
    lines.push(Line::raw(""));
    lines.push(section("LATEST CLEANUP PLAN"));
    if let Some(plan) = &app.cleanup_plan {
        lines.extend([
            field("Plan", &plan.plan_id),
            field("Status", &format!("{:?}", plan.status).to_lowercase()),
            field("Source revision", &plan.source_revision.to_string()),
            field("Files", &plan.items.len().to_string()),
            field("Bytes", &format_bytes(plan.reclaimable_bytes)),
            field("Created", &plan.created_at),
            field("Applied", plan.applied_at.as_deref().unwrap_or("not yet")),
            field("Restored", plan.restored_at.as_deref().unwrap_or("not yet")),
        ]);
    } else {
        lines.push(Line::styled("Create a plan to preview exact files", MUTED));
    }
    render_details(frame, area, "Storage and reversible cleanup", lines);
}

pub(super) fn render_history<B: TuiBackend>(frame: &mut Frame<'_>, area: Rect, app: &App<B>) {
    let rows = app
        .decisions
        .iter()
        .map(|decision| {
            Row::new(vec![
                Cell::from(decision.occurred_at.clone()),
                Cell::from(decision.kind.clone()),
                Cell::from(decision.subject_id.clone()),
                Cell::from(decision.command_id.clone()),
            ])
        })
        .collect();
    let details = app.selected_decision().map_or_else(
        || vec![Line::styled("No committed decisions", MUTED)],
        |decision| {
            vec![
                field("Event", &decision.event_id),
                field("Kind", &decision.kind),
                field("Subject", &decision.subject_id),
                field("Command", &decision.command_id),
                field("Occurred", &decision.occurred_at),
            ]
        },
    );
    render_table_detail(
        frame,
        area,
        &format!("Decision history ({})", app.decisions.len()),
        ["Time", "Kind", "Subject", "Command"],
        rows,
        [
            Constraint::Length(22),
            Constraint::Length(24),
            Constraint::Min(18),
            Constraint::Length(20),
        ],
        app.selection(),
        "Decision details",
        details,
    );
}
