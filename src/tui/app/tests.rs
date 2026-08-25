use super::*;
use crate::store::{CleanupItem, CleanupPlan, CleanupPlanStatus};
use crate::tui::backend::BackendReply;
use crate::tui::protocol::{
    BuildSummary, DiagnosticSummary, ProjectSummary, QueueJobSummary, QueueSummary, WorkerPayload,
};
use crossterm::event::KeyModifiers;

#[derive(Default)]
struct FakeBackend {
    snapshot: AppSnapshot,
    selected: Option<String>,
    projects: Vec<ProjectListItem>,
    dispatched: Vec<(WorkerCommand, u64)>,
    reply: Option<BackendReply>,
    dispatch_error: Option<&'static str>,
    refresh_error: bool,
    list_error: bool,
    select_error: bool,
    refreshes: usize,
    listings: usize,
}

impl TuiBackend for FakeBackend {
    fn refresh(&mut self) -> Result<AppSnapshot, BackendError> {
        self.refreshes += 1;
        if self.refresh_error {
            return Err(BackendError::Protocol("refresh failed".to_owned()));
        }
        Ok(self.snapshot.clone())
    }

    fn list_projects(&mut self) -> Result<Vec<ProjectListItem>, BackendError> {
        self.listings += 1;
        if self.list_error {
            return Err(BackendError::Protocol("list failed".to_owned()));
        }
        Ok(self.projects.clone())
    }

    fn select_project(&mut self, project_id: &str) -> Result<AppSnapshot, BackendError> {
        if self.select_error {
            return Err(BackendError::Protocol("select failed".to_owned()));
        }
        self.selected = Some(project_id.to_owned());
        self.snapshot.project.id = project_id.to_owned();
        Ok(self.snapshot.clone())
    }

    fn selected_project_id(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    fn dispatch(
        &mut self,
        command: WorkerCommand,
        expected_revision: u64,
    ) -> Result<BackendReply, BackendError> {
        self.dispatched.push((command, expected_revision));
        if let Some(error) = self.dispatch_error {
            return Err(match error {
                "revision" => BackendError::Worker {
                    code: "REVISION_CONFLICT".to_owned(),
                    message: "stale revision".to_owned(),
                    retryable: true,
                    current_revision: Some(9),
                },
                _ => BackendError::Protocol("dispatch failed".to_owned()),
            });
        }
        if let Some(reply) = self.reply.take() {
            return Ok(reply);
        }
        Ok(BackendReply {
            snapshot: Some(self.snapshot.clone()),
            payload: None,
            artifact_path: None,
            message: Some("ok".to_owned()),
        })
    }
}

fn snapshot() -> AppSnapshot {
    AppSnapshot {
        schema_version: "1.0".to_owned(),
        revision: 7,
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
        pending_approvals: vec![ApprovalSummary {
            approval_id: "APR-1".to_owned(),
            kind: "candidate_selection".to_owned(),
            shot_id: Some("S01".to_owned()),
            take_ids: vec!["S01-T001".to_owned()],
            blocking: true,
            description: "choose".to_owned(),
        }],
        queue: QueueSummary {
            revision: 2,
            paused: false,
            jobs: vec![QueueJobSummary {
                job_id: "JOB-1".to_owned(),
                subject: "S01".to_owned(),
                state: "queued".to_owned(),
                priority: "normal".to_owned(),
                resource: "gpu_exclusive".to_owned(),
                progress: None,
                eta_seconds: None,
            }],
        },
        shots: vec![ShotSummary {
            shot_id: "S01".to_owned(),
            title: "Door".to_owned(),
            stage: "candidates_ready".to_owned(),
            risk: "high".to_owned(),
            candidate_count: 1,
            ..ShotSummary::default()
        }],
        takes: vec![TakeSummary {
            take_id: "S01-T001".to_owned(),
            shot_id: "S01".to_owned(),
            status: "validated".to_owned(),
            profile: "audition".to_owned(),
            ..TakeSummary::default()
        }],
        builds: vec![BuildSummary {
            build_id: "BLD-1".to_owned(),
            kind: "draft".to_owned(),
            status: "ready".to_owned(),
            recipe: "builds/BLD-1/recipe.json".to_owned(),
            ..BuildSummary::default()
        }],
        diagnostics: vec![DiagnosticSummary {
            probe_id: "PROBE-1".to_owned(),
            component: "ffmpeg".to_owned(),
            status: "ready".to_owned(),
            summary: "available".to_owned(),
            capabilities: vec!["probe".to_owned()],
        }],
        ..AppSnapshot::default()
    }
}

fn backend() -> FakeBackend {
    FakeBackend {
        snapshot: snapshot(),
        selected: Some("rain-apartment".to_owned()),
        projects: vec![ProjectListItem {
            id: "rain-apartment".to_owned(),
            title: Some("Rain Apartment".to_owned()),
            paused: false,
            ..ProjectListItem::default()
        }],
        ..FakeBackend::default()
    }
}

fn cleanup_plan() -> CleanupPlan {
    CleanupPlan {
        schema_version: "1.0".to_owned(),
        plan_id: "CLN-1".to_owned(),
        project_id: "rain-apartment".to_owned(),
        source_revision: 7,
        status: CleanupPlanStatus::Planned,
        created_at: "100".to_owned(),
        applied_at: None,
        restored_at: None,
        active_operation: None,
        items: vec![CleanupItem {
            kind: "rejected_take".to_owned(),
            subject_id: "S01-T000".to_owned(),
            path: PathBuf::from("raw/S01/S01-T000.mp4"),
            bytes: 12,
        }],
        reclaimable_bytes: 12,
    }
}

fn command_for(app: &mut App<FakeBackend>, page: Page, key: char) -> WorkerCommand {
    app.switch_page(page);
    app.handle_key(KeyEvent::new(KeyCode::Char(key), KeyModifiers::NONE));
    app.confirmation
        .take()
        .unwrap_or_else(|| panic!("{} {key:?} should require confirmation", page.title()))
        .command
}

#[test]
fn shot_retry_requires_confirmation() {
    let mut app = App::new(backend(), Duration::from_secs(2));
    app.initial_refresh();
    app.switch_page(Page::Shots);

    app.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));
    assert!(app.confirmation.is_some());
    app.handle_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE));
    assert!(app.confirmation.is_none());
    assert_eq!(app.status.kind, StatusKind::Success);
    assert_eq!(
        app.backend.dispatched,
        vec![(
            WorkerCommand::RetryShot {
                shot_id: "S01".to_owned(),
            },
            7,
        )]
    );
}

#[test]
fn enter_on_shot_opens_filtered_takes() {
    let mut app = App::new(backend(), Duration::from_secs(2));
    app.initial_refresh();
    app.switch_page(Page::Shots);

    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(app.page, Page::Takes);
    assert_eq!(app.selected_shot_id.as_deref(), Some("S01"));
    assert_eq!(app.visible_takes().len(), 1);
}

#[test]
fn initial_refresh_selects_first_available_project() {
    let mut backend = backend();
    backend.selected = None;
    backend.projects = vec![ProjectListItem {
        id: "first-project".to_owned(),
        title: Some("First".to_owned()),
        ..ProjectListItem::default()
    }];
    let mut app = App::new(backend, Duration::from_secs(2));

    app.initial_refresh();

    assert_eq!(app.snapshot.as_ref().unwrap().project.id, "first-project");
    assert_eq!(app.page, Page::Dashboard);
    assert!(app.take_project_changed());
}

#[test]
fn batch_approval_cycles_take_and_explicitly_accepts_warnings() {
    let mut backend = backend();
    backend.snapshot.pending_approvals = vec![ApprovalSummary {
        approval_id: "APR-1".to_owned(),
        kind: "candidate_selection".to_owned(),
        shot_id: Some("S01".to_owned()),
        take_ids: vec!["S01-T001".to_owned(), "S01-T002".to_owned()],
        blocking: true,
        description: "choose".to_owned(),
    }];
    backend.snapshot.takes.push(TakeSummary {
        take_id: "S01-T002".to_owned(),
        shot_id: "S01".to_owned(),
        warnings: vec!["LIP_SYNC_REVIEW".to_owned()],
        ..TakeSummary::default()
    });
    let mut app = App::new(backend, Duration::from_secs(2));
    app.initial_refresh();
    app.switch_page(Page::Review);

    app.handle_key(KeyEvent::new(KeyCode::Char(']'), KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));

    let confirmation = app.confirmation.as_ref().unwrap();
    let WorkerCommand::ReviewBatch {
        selections,
        approve,
    } = &confirmation.command
    else {
        panic!("expected batch review command");
    };
    assert!(*approve);
    assert_eq!(selections.len(), 1);
    assert_eq!(selections[0].take_id, "S01-T002");
    assert!(selections[0].accept_warnings);
    assert!(confirmation.prompt.contains("accepts 1 warning"));
}

#[test]
fn page_actions_map_to_the_expected_worker_commands() {
    let mut app = App::new(backend(), Duration::from_secs(2));
    app.initial_refresh();
    app.cleanup_plan = Some(cleanup_plan());

    assert_eq!(
        command_for(&mut app, Page::Projects, ' '),
        WorkerCommand::PauseProject
    );
    assert!(matches!(
        command_for(&mut app, Page::Dashboard, 'a'),
        WorkerCommand::Approve { approval_id } if approval_id == "APR-1"
    ));
    assert!(matches!(
        command_for(&mut app, Page::Shots, 'u'),
        WorkerCommand::AuditionShot { shot_id } if shot_id == "S01"
    ));
    assert!(matches!(
        command_for(&mut app, Page::Shots, 'd'),
        WorkerCommand::RenderShot { shot_id } if shot_id == "S01"
    ));
    assert!(matches!(
        command_for(&mut app, Page::Shots, 'r'),
        WorkerCommand::RetryShot { shot_id } if shot_id == "S01"
    ));
    assert!(matches!(
        command_for(&mut app, Page::Takes, 's'),
        WorkerCommand::SelectTake { shot_id, take_id }
            if shot_id == "S01" && take_id == "S01-T001"
    ));
    assert!(matches!(
        command_for(&mut app, Page::Takes, 'a'),
        WorkerCommand::ApproveTake { shot_id, take_id }
            if shot_id == "S01" && take_id == "S01-T001"
    ));
    assert!(matches!(
        command_for(&mut app, Page::Takes, 'x'),
        WorkerCommand::RejectTake { shot_id, take_id }
            if shot_id == "S01" && take_id == "S01-T001"
    ));
    assert_eq!(
        command_for(&mut app, Page::Queue, ' '),
        WorkerCommand::PauseQueue
    );
    assert!(matches!(
        command_for(&mut app, Page::Queue, 'x'),
        WorkerCommand::CancelJob { job_id } if job_id == "JOB-1"
    ));
    assert!(matches!(
        command_for(&mut app, Page::Builds, 'b'),
        WorkerCommand::Build { kind, shot_ids }
            if kind == "draft" && shot_ids.is_empty()
    ));
    assert_eq!(
        command_for(&mut app, Page::Storage, 'p'),
        WorkerCommand::CreateCleanupPlan
    );
    assert!(matches!(
        command_for(&mut app, Page::Storage, 'a'),
        WorkerCommand::ApplyCleanupPlan { plan_id } if plan_id == "CLN-1"
    ));
    assert!(matches!(
        command_for(&mut app, Page::Storage, 'r'),
        WorkerCommand::RestoreCleanupPlan { plan_id } if plan_id == "CLN-1"
    ));
    assert!(matches!(
        command_for(&mut app, Page::Diagnostics, 'r'),
        WorkerCommand::RetryProbe { probe_id } if probe_id == "PROBE-1"
    ));
}

#[test]
fn read_only_page_actions_dispatch_immediately() {
    let mut app = App::new(backend(), Duration::from_secs(2));
    app.initial_refresh();

    for (page, key) in [
        (Page::Takes, 'p'),
        (Page::Builds, 'o'),
        (Page::Diagnostics, 'l'),
    ] {
        app.switch_page(page);
        app.handle_key(KeyEvent::new(KeyCode::Char(key), KeyModifiers::NONE));
    }

    assert_eq!(
        app.backend.dispatched,
        [
            (
                WorkerCommand::PreviewTake {
                    take_id: "S01-T001".to_owned(),
                },
                7,
            ),
            (
                WorkerCommand::OpenBuild {
                    build_id: "BLD-1".to_owned(),
                },
                7,
            ),
            (WorkerCommand::OpenLogs, 7),
        ]
    );
}

#[test]
fn navigation_selection_help_and_cancel_keys_are_bounded() {
    let mut app = App::new(backend(), Duration::from_secs(2));
    app.initial_refresh();

    app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert_eq!(app.page, Page::Review);
    app.handle_key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE));
    assert_eq!(app.page, Page::Dashboard);
    app.handle_key(KeyEvent::new(KeyCode::Char('0'), KeyModifiers::NONE));
    assert_eq!(app.page, Page::Diagnostics);
    app.handle_key(KeyEvent::new(KeyCode::Char('4'), KeyModifiers::NONE));
    assert_eq!(app.page, Page::Shots);
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(app.selection(), 0);
    app.handle_key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE));
    assert!(app.show_help);
    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(!app.show_help);

    app.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));
    assert!(app.confirmation.is_some());
    app.handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE));
    assert!(app.confirmation.is_none());
    assert_eq!(app.status.text, "Action cancelled");
    app.handle_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
    assert!(app.should_quit);
}

#[test]
fn actions_without_rows_report_what_is_missing() {
    let mut empty = backend();
    empty.snapshot.pending_approvals.clear();
    empty.snapshot.shots.clear();
    empty.snapshot.takes.clear();
    empty.snapshot.queue.jobs.clear();
    empty.snapshot.builds.clear();
    empty.snapshot.diagnostics.clear();
    let mut app = App::new(empty, Duration::from_secs(2));
    app.initial_refresh();

    for (page, key, expected) in [
        (Page::Dashboard, 'a', "No approval selected"),
        (Page::Shots, 'u', "No shot selected"),
        (Page::Takes, 's', "No take selected"),
        (Page::Queue, 'x', "No job selected"),
        (Page::Builds, 'o', "No build selected"),
        (Page::Storage, 'a', "No cleanup plan selected"),
        (Page::Diagnostics, 'r', "No diagnostic selected"),
    ] {
        app.switch_page(page);
        app.handle_key(KeyEvent::new(KeyCode::Char(key), KeyModifiers::NONE));
        assert_eq!(app.status.text, expected);
    }
}

#[test]
fn refresh_reconnect_and_empty_project_states_are_explicit() {
    let mut empty = backend();
    empty.selected = None;
    empty.projects.clear();
    let mut app = App::new(empty, Duration::from_millis(10));
    app.initial_refresh();
    assert_eq!(app.page, Page::Projects);
    assert_eq!(app.connection, ConnectionState::Connected);
    assert_eq!(app.status.text, "No projects available");

    app.refresh_now();
    assert!(app.backend.listings >= 3);
    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert_eq!(app.status.text, "No pending dialog");
    assert!(app.visible_takes().is_empty());

    let mut failing = backend();
    failing.refresh_error = true;
    let mut app = App::new(failing, Duration::from_millis(10));
    app.initial_refresh();
    assert!(matches!(app.connection, ConnectionState::Disconnected(_)));
    assert_eq!(app.status.text, "Worker unavailable; retrying");
    app.tick(Instant::now() + Duration::from_secs(30));
    assert!(app.backend.refreshes >= 2);

    app.backend.refresh_error = false;
    app.tick(Instant::now() + Duration::from_secs(120));
    assert_eq!(app.connection, ConnectionState::Connected);
    assert_eq!(app.status.text, "Snapshot refreshed");
}

#[test]
fn project_review_and_open_actions_cover_boundary_states() {
    let mut app = App::new(backend(), Duration::from_secs(2));
    app.initial_refresh();

    app.switch_page(Page::Projects);
    app.projects.clear();
    app.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
    assert_eq!(app.status.text, "No project selected");

    app.projects = vec![ProjectListItem {
        id: "other-project".to_owned(),
        ..ProjectListItem::default()
    }];
    app.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
    assert!(app.status.text.starts_with("Open the highlighted project"));

    app.projects[0].id = "rain-apartment".to_owned();
    app.projects[0].paused = true;
    app.backend.projects[0].paused = true;
    assert_eq!(
        command_for(&mut app, Page::Projects, ' '),
        WorkerCommand::ResumeProject
    );

    app.switch_page(Page::Review);
    app.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE));
    let confirmation = app.confirmation.take().unwrap();
    assert!(confirmation.prompt.starts_with("Select 1"));
    app.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE));
    assert_eq!(app.status.text, "No included review row selected");

    app.snapshot.as_mut().unwrap().pending_approvals.clear();
    for key in [' ', '['] {
        app.handle_key(KeyEvent::new(KeyCode::Char(key), KeyModifiers::NONE));
        assert_eq!(app.status.text, "No review row selected");
    }

    let mut no_shot = snapshot();
    no_shot.pending_approvals[0].shot_id = None;
    app.sync_review_choices(&no_shot);
    assert!(app.review_rows().is_empty());

    app.snapshot = None;
    app.switch_page(Page::Queue);
    app.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
    assert_eq!(app.status.text, "No queue selected");
}

#[test]
fn enter_navigation_opens_each_supported_selection() {
    let mut app = App::new(backend(), Duration::from_secs(2));
    app.initial_refresh();

    for page in [Page::Dashboard, Page::Review, Page::Shots] {
        app.switch_page(page);
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.page, Page::Takes);
        assert_eq!(app.selected_shot_id.as_deref(), Some("S01"));
    }

    app.switch_page(Page::Takes);
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    app.switch_page(Page::Builds);
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(app.backend.dispatched.len(), 2);

    app.switch_page(Page::Projects);
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(app.page, Page::Dashboard);
}

#[test]
fn payloads_artifacts_and_dispatch_failures_update_app_state() {
    let mut app = App::new(backend(), Duration::from_secs(2));
    app.initial_refresh();

    app.apply_payload(WorkerPayload::ProjectList {
        projects: vec![ProjectListItem {
            id: "listed".to_owned(),
            ..ProjectListItem::default()
        }],
    });
    app.apply_payload(WorkerPayload::StorageReport(StorageReport {
        project_id: "rain-apartment".to_owned(),
        total_bytes: 100,
        trash_bytes: 10,
        reclaimable_bytes: 12,
        reclaimable_files: 1,
    }));
    app.apply_payload(WorkerPayload::CleanupPlan(cleanup_plan()));
    app.apply_payload(WorkerPayload::DecisionHistory {
        decisions: vec![DecisionRecord {
            event_id: "EVT-1".to_owned(),
            kind: "take_selected".to_owned(),
            subject_id: "S01-T001".to_owned(),
            command_id: "CMD-1".to_owned(),
            occurred_at: "100".to_owned(),
        }],
    });
    assert_eq!(app.projects[0].id, "listed");
    assert_eq!(app.storage_report.as_ref().unwrap().total_bytes, 100);
    assert_eq!(app.cleanup_plan.as_ref().unwrap().plan_id, "CLN-1");
    assert_eq!(app.selected_decision().unwrap().event_id, "EVT-1");

    app.backend.reply = Some(BackendReply {
        snapshot: None,
        payload: None,
        artifact_path: Some(PathBuf::from("review/take.mp4")),
        message: None,
    });
    app.dispatch(WorkerCommand::OpenLogs);
    assert_eq!(app.status.text, "Command accepted");
    assert_eq!(
        app.take_pending_artifact(),
        Some(PathBuf::from("review/take.mp4"))
    );
    assert!(app.take_pending_artifact().is_none());

    let refreshes = app.backend.refreshes;
    app.backend.reply = Some(BackendReply {
        snapshot: None,
        payload: None,
        artifact_path: None,
        message: Some("queued".to_owned()),
    });
    app.dispatch(WorkerCommand::Build {
        kind: "draft".to_owned(),
        shot_ids: Vec::new(),
    });
    assert!(app.backend.refreshes > refreshes);

    app.backend.dispatch_error = Some("revision");
    app.dispatch(WorkerCommand::PauseQueue);
    assert!(app.backend.refreshes > refreshes + 1);

    app.backend.dispatch_error = Some("protocol");
    app.dispatch(WorkerCommand::OpenLogs);
    assert!(matches!(app.connection, ConnectionState::Disconnected(_)));
    assert!(app.status.text.contains("dispatch failed"));

    app.snapshot = None;
    let dispatched = app.backend.dispatched.len();
    app.dispatch(WorkerCommand::OpenLogs);
    assert_eq!(app.backend.dispatched.len(), dispatched);
    assert_eq!(app.status.text, "No worker snapshot; action not sent");
}

#[test]
fn backend_listing_and_selection_errors_are_visible() {
    let mut failing = backend();
    failing.list_error = true;
    let mut app = App::new(failing, Duration::from_secs(2));
    app.page = Page::Projects;
    assert!(!app.refresh_projects());
    assert!(app.status.text.contains("list failed"));

    app.backend.list_error = false;
    app.backend.select_error = true;
    app.select_project("rain-apartment");
    assert!(app.status.text.contains("select failed"));
    assert!(matches!(app.connection, ConnectionState::Disconnected(_)));
}
