use super::*;
use crate::tui::backend::BackendReply;
use crate::tui::protocol::{ProjectSummary, QueueSummary};
use crossterm::event::KeyModifiers;

#[derive(Default)]
struct FakeBackend {
    snapshot: AppSnapshot,
    selected: Option<String>,
    projects: Vec<ProjectListItem>,
    dispatched: Vec<(WorkerCommand, u64)>,
}

impl TuiBackend for FakeBackend {
    fn refresh(&mut self) -> Result<AppSnapshot, BackendError> {
        Ok(self.snapshot.clone())
    }

    fn list_projects(&mut self) -> Result<Vec<ProjectListItem>, BackendError> {
        Ok(self.projects.clone())
    }

    fn select_project(&mut self, project_id: &str) -> Result<AppSnapshot, BackendError> {
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
        queue: QueueSummary::default(),
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
        ..AppSnapshot::default()
    }
}

fn backend() -> FakeBackend {
    FakeBackend {
        snapshot: snapshot(),
        selected: Some("rain-apartment".to_owned()),
        ..FakeBackend::default()
    }
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
