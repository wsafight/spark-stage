use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

use super::*;
use crate::store::{CleanupItem, CleanupPlan, CleanupPlanStatus, DecisionRecord, StorageReport};
use crate::tui::backend::{BackendError, BackendReply};
use crate::tui::protocol::{
    AppSnapshot, BudgetSummary, DiagnosticSummary, FailureSummary, GpuSummary, ProjectListItem,
    ProjectSummary, QueueJobSummary, QueueSummary, WorkerCommand,
};

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
            payload: None,
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
            paused: false,
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
            ..BudgetSummary::default()
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
            revision: 3,
            paused: false,
            jobs: vec![QueueJobSummary {
                job_id: "JOB-QUEUE-0001".to_owned(),
                subject: "S02 audition".to_owned(),
                state: "running".to_owned(),
                priority: "normal".to_owned(),
                resource: "gpu_exclusive".to_owned(),
                progress: Some(0.5),
                eta_seconds: Some(65),
            }],
        },
        builds: vec![BuildSummary {
            build_id: "BLD-DRAFT-0001".to_owned(),
            kind: "draft".to_owned(),
            status: "ready".to_owned(),
            recipe: "builds/BLD-DRAFT-0001/recipe.json".to_owned(),
            command_id: "CMD-BUILD-0001".to_owned(),
            output_path: Some("builds/BLD-DRAFT-0001/draft.mp4".into()),
            warnings: vec!["LOUDNESS_REVIEW".to_owned()],
            stale: false,
        }],
        diagnostics: vec![DiagnosticSummary {
            probe_id: "PROBE-FFMPEG".to_owned(),
            component: "ffmpeg".to_owned(),
            status: "ready".to_owned(),
            summary: "media tools available".to_owned(),
            capabilities: vec!["ffprobe".to_owned(), "blackdetect".to_owned()],
        }],
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

#[test]
fn stale_build_status_is_explicit() {
    let build = BuildSummary {
        status: "needs_review".to_owned(),
        stale: true,
        ..BuildSummary::default()
    };

    assert_eq!(build_status(&build), "needs_review (stale)");
}

#[test]
fn every_operational_page_renders_populated_details() {
    let mut app = app();
    app.projects = vec![ProjectListItem {
        id: "rain-apartment".to_owned(),
        title: Some("Rain Apartment".to_owned()),
        stage: Some("shooting".to_owned()),
        outcome: Some("needs_review".to_owned()),
        paused: true,
        revision: Some(42),
        updated_at: Some("2026-08-25T12:00:00Z".to_owned()),
        error: None,
    }];
    app.storage_report = Some(StorageReport {
        project_id: "rain-apartment".to_owned(),
        total_bytes: 4 * 1024 * 1024,
        trash_bytes: 1024,
        reclaimable_bytes: 2048,
        reclaimable_files: 1,
    });
    app.cleanup_plan = Some(CleanupPlan {
        schema_version: "1.0".to_owned(),
        plan_id: "CLN-REVIEW-0001".to_owned(),
        project_id: "rain-apartment".to_owned(),
        source_revision: 42,
        status: CleanupPlanStatus::Planned,
        created_at: "2026-08-25T12:00:00Z".to_owned(),
        applied_at: None,
        restored_at: None,
        active_operation: None,
        items: vec![CleanupItem {
            kind: "rejected_take".to_owned(),
            subject_id: "S01-T001".to_owned(),
            path: "raw/S01/S01-T001.mp4".into(),
            bytes: 2048,
        }],
        reclaimable_bytes: 2048,
    });
    app.decisions = vec![DecisionRecord {
        event_id: "EVT-SELECT-0001".to_owned(),
        kind: "take_selected".to_owned(),
        subject_id: "S01-T002".to_owned(),
        command_id: "CMD-SELECT-0001".to_owned(),
        occurred_at: "2026-08-25T12:00:00Z".to_owned(),
    }];

    for (page, markers) in [
        (Page::Projects, ["Projects (1)", "rain-apartment"]),
        (Page::Review, ["Batch review", "S01-T002"]),
        (Page::Takes, ["Takes for S01", "duration_ok"]),
        (Page::Queue, ["JOB-QUEUE-0001", "gpu_exclusive"]),
        (Page::Builds, ["BLD-DRAFT-0001", "LOUDNESS_REVIEW"]),
        (Page::Storage, ["CLN-REVIEW-0001", "2048 B"]),
        (Page::History, ["EVT-SELECT-0001", "take_selected"]),
        (Page::Diagnostics, ["PROBE-FFMPEG", "blackdetect"]),
    ] {
        app.page = page;
        let output = draw(&app, 160, 40);
        for marker in markers {
            assert!(
                output.contains(marker),
                "{} page did not render {marker:?}",
                page.title()
            );
        }
    }
}

#[test]
fn help_and_disconnected_states_remain_actionable() {
    let backend = RenderBackend {
        snapshot: snapshot(),
    };
    let mut app = App::new(backend, Duration::from_secs(2));
    app.page = Page::Dashboard;

    let disconnected = draw(&app, 100, 30);
    assert!(disconnected.contains("Worker unavailable"));
    assert!(disconnected.contains("Press g to retry"));

    app.show_help = true;
    let help = draw(&app, 100, 30);
    assert!(help.contains("Mutations require"));
    assert!(help.contains("Storage: p plan"));
}
