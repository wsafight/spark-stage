use std::path::PathBuf;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent};

use super::backend::{BackendError, TuiBackend};
use super::protocol::{
    AppSnapshot, ApprovalSummary, BuildSummary, DiagnosticSummary, QueueJobSummary, ShotSummary,
    TakeSummary, WorkerCommand,
};

const MAX_RECONNECT_DELAY: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Dashboard,
    Shots,
    Takes,
    Queue,
    Builds,
    Diagnostics,
}

impl Page {
    pub const ALL: [Self; 6] = [
        Self::Dashboard,
        Self::Shots,
        Self::Takes,
        Self::Queue,
        Self::Builds,
        Self::Diagnostics,
    ];

    #[must_use]
    pub const fn title(self) -> &'static str {
        match self {
            Self::Dashboard => "Dashboard",
            Self::Shots => "Shots",
            Self::Takes => "Takes",
            Self::Queue => "Queue",
            Self::Builds => "Builds",
            Self::Diagnostics => "Diagnostics",
        }
    }

    pub(crate) const fn index(self) -> usize {
        match self {
            Self::Dashboard => 0,
            Self::Shots => 1,
            Self::Takes => 2,
            Self::Queue => 3,
            Self::Builds => 4,
            Self::Diagnostics => 5,
        }
    }

    fn next(self) -> Self {
        Self::ALL[(self.index() + 1) % Self::ALL.len()]
    }

    fn previous(self) -> Self {
        Self::ALL[(self.index() + Self::ALL.len() - 1) % Self::ALL.len()]
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionState {
    Connected,
    Disconnected(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusKind {
    Info,
    Success,
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusMessage {
    pub kind: StatusKind,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct Confirmation {
    pub prompt: String,
    pub command: WorkerCommand,
}

pub struct App<B> {
    backend: B,
    pub snapshot: Option<AppSnapshot>,
    pub connection: ConnectionState,
    pub page: Page,
    selections: [usize; Page::ALL.len()],
    pub selected_shot_id: Option<String>,
    pub status: StatusMessage,
    pub confirmation: Option<Confirmation>,
    pub show_help: bool,
    pub should_quit: bool,
    pending_artifact: Option<PathBuf>,
    refresh_interval: Duration,
    next_refresh_at: Instant,
    reconnect_delay: Duration,
}

impl<B: TuiBackend> App<B> {
    pub fn new(backend: B, refresh_interval: Duration) -> Self {
        let now = Instant::now();
        Self {
            backend,
            snapshot: None,
            connection: ConnectionState::Disconnected("connecting".to_owned()),
            page: Page::Dashboard,
            selections: [0; Page::ALL.len()],
            selected_shot_id: None,
            status: StatusMessage {
                kind: StatusKind::Info,
                text: "Connecting to worker".to_owned(),
            },
            confirmation: None,
            show_help: false,
            should_quit: false,
            pending_artifact: None,
            refresh_interval,
            next_refresh_at: now,
            reconnect_delay: Duration::from_millis(500),
        }
    }

    pub fn initial_refresh(&mut self) {
        self.refresh(Instant::now());
    }

    pub fn tick(&mut self, now: Instant) {
        if now >= self.next_refresh_at {
            self.refresh(now);
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        if self.confirmation.is_some() {
            self.handle_confirmation_key(key);
            return;
        }
        if self.show_help {
            if matches!(
                key.code,
                KeyCode::Esc | KeyCode::Char('?') | KeyCode::Char('q')
            ) {
                self.show_help = false;
            }
            return;
        }

        match key.code {
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Char('?') => self.show_help = true,
            KeyCode::Char('g') => self.refresh(Instant::now()),
            KeyCode::Tab => self.switch_page(self.page.next()),
            KeyCode::BackTab => self.switch_page(self.page.previous()),
            KeyCode::Left if key.modifiers.is_empty() => self.switch_page(self.page.previous()),
            KeyCode::Right if key.modifiers.is_empty() => self.switch_page(self.page.next()),
            KeyCode::Char(character @ '1'..='6') => {
                let index = usize::from(character as u8 - b'1');
                self.switch_page(Page::ALL[index]);
            }
            KeyCode::Up | KeyCode::Char('k') => self.move_selection(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_selection(1),
            KeyCode::Enter => self.open_selected(),
            KeyCode::Char(character) => self.handle_page_action(character),
            KeyCode::Esc => {
                self.status = StatusMessage {
                    kind: StatusKind::Info,
                    text: "No pending dialog".to_owned(),
                }
            }
            _ => {}
        }
    }

    #[must_use]
    pub fn selection(&self) -> usize {
        self.selections[self.page.index()]
    }

    #[must_use]
    pub fn selected_approval(&self) -> Option<&ApprovalSummary> {
        self.snapshot
            .as_ref()?
            .pending_approvals
            .get(self.selections[Page::Dashboard.index()])
    }

    #[must_use]
    pub fn selected_shot(&self) -> Option<&ShotSummary> {
        self.snapshot
            .as_ref()?
            .shots
            .get(self.selections[Page::Shots.index()])
    }

    #[must_use]
    pub fn visible_takes(&self) -> Vec<&TakeSummary> {
        let Some(snapshot) = &self.snapshot else {
            return Vec::new();
        };
        snapshot
            .takes
            .iter()
            .filter(|take| {
                self.selected_shot_id
                    .as_ref()
                    .is_none_or(|shot_id| &take.shot_id == shot_id)
            })
            .collect()
    }

    #[must_use]
    pub fn selected_take(&self) -> Option<&TakeSummary> {
        self.visible_takes()
            .get(self.selections[Page::Takes.index()])
            .copied()
    }

    #[must_use]
    pub fn selected_queue_job(&self) -> Option<&QueueJobSummary> {
        self.snapshot
            .as_ref()?
            .queue
            .jobs
            .get(self.selections[Page::Queue.index()])
    }

    #[must_use]
    pub fn selected_build(&self) -> Option<&BuildSummary> {
        self.snapshot
            .as_ref()?
            .builds
            .get(self.selections[Page::Builds.index()])
    }

    #[must_use]
    pub fn selected_diagnostic(&self) -> Option<&DiagnosticSummary> {
        self.snapshot
            .as_ref()?
            .diagnostics
            .get(self.selections[Page::Diagnostics.index()])
    }

    pub fn take_pending_artifact(&mut self) -> Option<PathBuf> {
        self.pending_artifact.take()
    }

    fn refresh(&mut self, now: Instant) {
        match self.backend.refresh() {
            Ok(snapshot) => {
                self.apply_snapshot(snapshot);
                self.connection = ConnectionState::Connected;
                self.status = StatusMessage {
                    kind: StatusKind::Success,
                    text: "Snapshot refreshed".to_owned(),
                };
                self.reconnect_delay = Duration::from_millis(500);
                self.next_refresh_at = now + self.refresh_interval;
            }
            Err(error) => {
                self.connection = ConnectionState::Disconnected(error.to_string());
                self.status = StatusMessage {
                    kind: StatusKind::Warning,
                    text: "Worker unavailable; retrying".to_owned(),
                };
                self.next_refresh_at = now + self.reconnect_delay;
                self.reconnect_delay = (self.reconnect_delay * 2).min(MAX_RECONNECT_DELAY);
            }
        }
    }

    fn apply_snapshot(&mut self, snapshot: AppSnapshot) {
        if self.selected_shot_id.is_none() {
            self.selected_shot_id = snapshot.shots.first().map(|shot| shot.shot_id.clone());
        } else if !snapshot.shots.iter().any(|shot| {
            self.selected_shot_id
                .as_ref()
                .is_some_and(|selected| selected == &shot.shot_id)
        }) {
            self.selected_shot_id = snapshot.shots.first().map(|shot| shot.shot_id.clone());
        }
        self.snapshot = Some(snapshot);
        self.clamp_all_selections();
    }

    fn switch_page(&mut self, page: Page) {
        self.page = page;
        self.clamp_selection(page);
    }

    fn move_selection(&mut self, delta: isize) {
        let count = self.row_count(self.page);
        let selected = &mut self.selections[self.page.index()];
        if count == 0 {
            *selected = 0;
            return;
        }
        *selected = selected.saturating_add_signed(delta).min(count - 1);
        if self.page == Page::Shots {
            self.selected_shot_id = self.selected_shot().map(|shot| shot.shot_id.clone());
            self.clamp_selection(Page::Takes);
        }
    }

    fn row_count(&self, page: Page) -> usize {
        let Some(snapshot) = &self.snapshot else {
            return 0;
        };
        match page {
            Page::Dashboard => snapshot.pending_approvals.len(),
            Page::Shots => snapshot.shots.len(),
            Page::Takes => self.visible_takes().len(),
            Page::Queue => snapshot.queue.jobs.len(),
            Page::Builds => snapshot.builds.len(),
            Page::Diagnostics => snapshot.diagnostics.len(),
        }
    }

    fn clamp_all_selections(&mut self) {
        for page in Page::ALL {
            self.clamp_selection(page);
        }
    }

    fn clamp_selection(&mut self, page: Page) {
        let count = self.row_count(page);
        let selected = &mut self.selections[page.index()];
        *selected = if count == 0 {
            0
        } else {
            (*selected).min(count - 1)
        };
    }

    fn open_selected(&mut self) {
        match self.page {
            Page::Dashboard => {
                if let Some(shot_id) = self
                    .selected_approval()
                    .and_then(|approval| approval.shot_id.clone())
                {
                    self.select_shot_and_open_takes(&shot_id);
                }
            }
            Page::Shots => {
                if let Some(shot_id) = self.selected_shot().map(|shot| shot.shot_id.clone()) {
                    self.select_shot_and_open_takes(&shot_id);
                }
            }
            Page::Takes => self.preview_selected_take(),
            Page::Builds => self.open_selected_build(),
            _ => {}
        }
    }

    fn select_shot_and_open_takes(&mut self, shot_id: &str) {
        self.selected_shot_id = Some(shot_id.to_owned());
        self.switch_page(Page::Takes);
    }

    fn handle_page_action(&mut self, character: char) {
        match (self.page, character) {
            (Page::Dashboard, 'a') => self.confirm_selected_approval(),
            (Page::Shots, 'u') => self.confirm_shot_command("Start audition", |shot_id| {
                WorkerCommand::AuditionShot { shot_id }
            }),
            (Page::Shots, 'd') => self.confirm_shot_command("Render final take", |shot_id| {
                WorkerCommand::RenderShot { shot_id }
            }),
            (Page::Shots, 'r') => self
                .confirm_shot_command("Retry shot", |shot_id| WorkerCommand::RetryShot { shot_id }),
            (Page::Takes, 's') => self
                .confirm_take_command("Select candidate", |shot_id, take_id| {
                    WorkerCommand::SelectTake { shot_id, take_id }
                }),
            (Page::Takes, 'a') => self.confirm_take_command("Approve take", |shot_id, take_id| {
                WorkerCommand::ApproveTake { shot_id, take_id }
            }),
            (Page::Takes, 'x') => self.confirm_take_command("Reject take", |shot_id, take_id| {
                WorkerCommand::RejectTake { shot_id, take_id }
            }),
            (Page::Takes, 'p') => self.preview_selected_take(),
            (Page::Queue, ' ') => self.confirm_queue_toggle(),
            (Page::Queue, 'x') => self.confirm_cancel_job(),
            (Page::Builds, 'b') => self.confirm_build(),
            (Page::Builds, 'o') => self.open_selected_build(),
            (Page::Diagnostics, 'r') => self.confirm_retry_probe(),
            (Page::Diagnostics, 'l') => self.dispatch_read_only(WorkerCommand::OpenLogs),
            _ => {}
        }
    }

    fn confirm_selected_approval(&mut self) {
        let Some(approval) = self.selected_approval() else {
            self.no_selection("approval");
            return;
        };
        self.confirmation = Some(Confirmation {
            prompt: format!("Approve {} ({})?", approval.kind, approval.approval_id),
            command: WorkerCommand::Approve {
                approval_id: approval.approval_id.clone(),
            },
        });
    }

    fn confirm_shot_command(&mut self, label: &str, command: impl FnOnce(String) -> WorkerCommand) {
        let Some(shot_id) = self.selected_shot().map(|shot| shot.shot_id.clone()) else {
            self.no_selection("shot");
            return;
        };
        self.confirmation = Some(Confirmation {
            prompt: format!("{label} {shot_id}?"),
            command: command(shot_id),
        });
    }

    fn confirm_take_command(
        &mut self,
        label: &str,
        command: impl FnOnce(String, String) -> WorkerCommand,
    ) {
        let Some(take) = self
            .selected_take()
            .map(|take| (take.shot_id.clone(), take.take_id.clone()))
        else {
            self.no_selection("take");
            return;
        };
        self.confirmation = Some(Confirmation {
            prompt: format!("{label} {}?", take.1),
            command: command(take.0, take.1),
        });
    }

    fn confirm_queue_toggle(&mut self) {
        let Some(snapshot) = &self.snapshot else {
            self.no_selection("queue");
            return;
        };
        let (prompt, command) = if snapshot.queue.paused {
            ("Resume queue?", WorkerCommand::ResumeQueue)
        } else {
            (
                "Pause queue after the current safe boundary?",
                WorkerCommand::PauseQueue,
            )
        };
        self.confirmation = Some(Confirmation {
            prompt: prompt.to_owned(),
            command,
        });
    }

    fn confirm_cancel_job(&mut self) {
        let Some(job_id) = self.selected_queue_job().map(|job| job.job_id.clone()) else {
            self.no_selection("job");
            return;
        };
        self.confirmation = Some(Confirmation {
            prompt: format!("Cancel job {job_id}? Existing outputs remain."),
            command: WorkerCommand::CancelJob { job_id },
        });
    }

    fn confirm_build(&mut self) {
        let kind = self
            .selected_build()
            .map_or_else(|| "draft".to_owned(), |build| build.kind.clone());
        self.confirmation = Some(Confirmation {
            prompt: format!("Build or rebuild {kind}?"),
            command: WorkerCommand::Build { kind },
        });
    }

    fn confirm_retry_probe(&mut self) {
        let Some(probe_id) = self
            .selected_diagnostic()
            .map(|diagnostic| diagnostic.probe_id.clone())
        else {
            self.no_selection("diagnostic");
            return;
        };
        self.confirmation = Some(Confirmation {
            prompt: format!("Retry probe {probe_id}?"),
            command: WorkerCommand::RetryProbe { probe_id },
        });
    }

    fn preview_selected_take(&mut self) {
        let Some(take_id) = self.selected_take().map(|take| take.take_id.clone()) else {
            self.no_selection("take");
            return;
        };
        self.dispatch_read_only(WorkerCommand::PreviewTake { take_id });
    }

    fn open_selected_build(&mut self) {
        let Some(build_id) = self.selected_build().map(|build| build.build_id.clone()) else {
            self.no_selection("build");
            return;
        };
        self.dispatch_read_only(WorkerCommand::OpenBuild { build_id });
    }

    fn handle_confirmation_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('y') | KeyCode::Enter => {
                let confirmation = self.confirmation.take().expect("checked above");
                self.dispatch(confirmation.command);
            }
            KeyCode::Char('n') | KeyCode::Esc => {
                self.confirmation = None;
                self.status = StatusMessage {
                    kind: StatusKind::Info,
                    text: "Action cancelled".to_owned(),
                };
            }
            _ => {}
        }
    }

    fn dispatch_read_only(&mut self, command: WorkerCommand) {
        debug_assert!(!command.is_mutating());
        self.dispatch(command);
    }

    fn dispatch(&mut self, command: WorkerCommand) {
        let Some(revision) = self.snapshot.as_ref().map(|snapshot| snapshot.revision) else {
            self.status = StatusMessage {
                kind: StatusKind::Error,
                text: "No worker snapshot; action not sent".to_owned(),
            };
            return;
        };

        match self.backend.dispatch(command, revision) {
            Ok(reply) => {
                if let Some(snapshot) = reply.snapshot {
                    self.apply_snapshot(snapshot);
                } else {
                    self.refresh(Instant::now());
                }
                self.pending_artifact = reply.artifact_path;
                self.status = StatusMessage {
                    kind: StatusKind::Success,
                    text: reply
                        .message
                        .unwrap_or_else(|| "Command accepted".to_owned()),
                };
            }
            Err(error) if error.is_revision_conflict() => {
                let revision = error
                    .current_revision()
                    .map_or_else(|| "unknown".to_owned(), |value| value.to_string());
                self.status = StatusMessage {
                    kind: StatusKind::Warning,
                    text: format!(
                        "Revision changed to {revision}; refreshed without replaying action"
                    ),
                };
                self.refresh(Instant::now());
            }
            Err(error) => self.record_command_error(error),
        }
    }

    fn record_command_error(&mut self, error: BackendError) {
        let disconnected = matches!(
            error,
            BackendError::Connect { .. }
                | BackendError::Io(_)
                | BackendError::Frame(_)
                | BackendError::Protocol(_)
        );
        if disconnected {
            self.connection = ConnectionState::Disconnected(error.to_string());
            self.next_refresh_at = Instant::now() + self.reconnect_delay;
        }
        self.status = StatusMessage {
            kind: StatusKind::Error,
            text: error.to_string(),
        };
    }

    fn no_selection(&mut self, subject: &str) {
        self.status = StatusMessage {
            kind: StatusKind::Warning,
            text: format!("No {subject} selected"),
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::backend::BackendReply;
    use crate::tui::protocol::{ProjectSummary, QueueSummary};
    use crossterm::event::KeyModifiers;

    #[derive(Default)]
    struct FakeBackend {
        snapshot: AppSnapshot,
        dispatched: Vec<(WorkerCommand, u64)>,
    }

    impl TuiBackend for FakeBackend {
        fn refresh(&mut self) -> Result<AppSnapshot, BackendError> {
            Ok(self.snapshot.clone())
        }

        fn dispatch(
            &mut self,
            command: WorkerCommand,
            expected_revision: u64,
        ) -> Result<BackendReply, BackendError> {
            self.dispatched.push((command, expected_revision));
            Ok(BackendReply {
                snapshot: Some(self.snapshot.clone()),
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

    #[test]
    fn shot_retry_requires_confirmation() {
        let backend = FakeBackend {
            snapshot: snapshot(),
            ..FakeBackend::default()
        };
        let mut app = App::new(backend, Duration::from_secs(2));
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
        let backend = FakeBackend {
            snapshot: snapshot(),
            ..FakeBackend::default()
        };
        let mut app = App::new(backend, Duration::from_secs(2));
        app.initial_refresh();
        app.switch_page(Page::Shots);

        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.page, Page::Takes);
        assert_eq!(app.selected_shot_id.as_deref(), Some("S01"));
        assert_eq!(app.visible_takes().len(), 1);
    }
}
