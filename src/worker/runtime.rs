use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Component, Path, PathBuf};
use std::sync::mpsc::TryRecvError;
use std::thread;
use std::time::{Duration, Instant};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use ulid::Ulid;

use crate::adapter::{CameraAdapter, CancelOutcome, ComfyAdapter, ComfyAdapterConfig};
use crate::build::{BuildEvent, BuildExecutorHandle, BuildRecipe, BuildRequest};
use crate::domain::{
    Approval, ApprovalKind, AttemptJournal, AttemptState, FailureRecord, JobJournal, JobState,
    Operation, ProjectOutcome, ProjectStage, ProjectState, PromotionStrategy, QualityTarget,
    QueueEntry, QueueState, Risk, ShotStage, TakeMetadata, WorkMode,
};
use crate::ipc::{
    AppSnapshot, ApprovalSummary, BudgetSummary, BuildSummary, ClientRequest, DiagnosticSummary,
    FailureSummary, GpuSummary, IPC_PROTOCOL_VERSION, ProjectSummary, QueueJobSummary,
    QueueSummary, RevisionEvent, ShotSummary, TakeSummary, WorkerCommand, WorkerError, WorkerReply,
    read_frame, write_frame,
};
use crate::paths::AppPaths;
use crate::store::{
    ExclusiveFileLock, ProjectStore, StoreError, append_jsonl, read_json_if_exists, read_jsonl,
    sha256_json, write_json_atomic,
};
use crate::validation::validate_json;

use super::executor::{
    ExecutionContext, ExecutorCancellation, ExecutorEvent, ExecutorHandle, ExecutorRequest,
};

mod auditions;
mod builds;
mod commands;
mod execution;
mod snapshot;
mod support;

use support::*;

#[derive(Debug, Clone)]
pub struct WorkerOptions {
    pub paths: AppPaths,
    pub adapter_config: Option<PathBuf>,
}

pub fn run(options: WorkerOptions) -> Result<(), WorkerRunError> {
    let _worker_lock = ExclusiveFileLock::acquire(&options.paths.worker_lock())?;
    let mut runtime =
        WorkerRuntime::open_with_adapter(options.paths.clone(), options.adapter_config)?;
    prepare_socket(&options.paths.socket)?;
    let listener =
        UnixListener::bind(&options.paths.socket).map_err(|source| WorkerRunError::Socket {
            path: options.paths.socket.clone(),
            source,
        })?;
    fs::set_permissions(&options.paths.socket, fs::Permissions::from_mode(0o600)).map_err(
        |source| WorkerRunError::Socket {
            path: options.paths.socket.clone(),
            source,
        },
    )?;
    let _socket_guard = SocketGuard(options.paths.socket.clone());
    listener
        .set_nonblocking(true)
        .map_err(|source| WorkerRunError::Socket {
            path: options.paths.socket.clone(),
            source,
        })?;
    let executor = ExecutorHandle::spawn().map_err(WorkerRunError::Executor)?;
    runtime.camera_cancellation = Some(executor.cancellation());
    let mut executor_busy = false;
    let mut dispatch_after = Instant::now();
    let mut subscribers = Vec::new();

    loop {
        if runtime.poll_build_events()? {
            notify_subscribers(&runtime, &mut subscribers);
        }
        loop {
            match executor.try_recv() {
                Ok(event) => {
                    let finishes_request = event.finishes_request();
                    let retry_delay = event.retry_delay();
                    let queue_revision = runtime.queue.revision;
                    match runtime.apply_executor_event(event) {
                        Ok(Some(request)) => {
                            executor.send(request).map_err(|error| {
                                WorkerRunError::ExecutorChannel(error.to_string())
                            })?;
                            executor_busy = true;
                            if runtime.queue.revision != queue_revision {
                                notify_subscribers(&runtime, &mut subscribers);
                            }
                        }
                        Ok(None) => {
                            if finishes_request {
                                executor_busy = false;
                                dispatch_after = Instant::now() + retry_delay;
                            }
                            if runtime.queue.revision != queue_revision {
                                notify_subscribers(&runtime, &mut subscribers);
                            }
                        }
                        Err(error) => {
                            eprintln!("camera event commit failed: {error}");
                            if finishes_request {
                                executor_busy = false;
                                dispatch_after = Instant::now() + Duration::from_secs(5);
                            }
                        }
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    return Err(WorkerRunError::ExecutorChannel(
                        "camera executor stopped".to_owned(),
                    ));
                }
            }
        }
        if !executor_busy && Instant::now() >= dispatch_after {
            let queue_revision = runtime.queue.revision;
            match runtime.next_executor_request() {
                Ok(Some(request)) => {
                    executor
                        .send(request)
                        .map_err(|error| WorkerRunError::ExecutorChannel(error.to_string()))?;
                    executor_busy = true;
                    if runtime.queue.revision != queue_revision {
                        notify_subscribers(&runtime, &mut subscribers);
                    }
                }
                Ok(None) => {
                    if runtime.queue.revision != queue_revision {
                        notify_subscribers(&runtime, &mut subscribers);
                    }
                }
                Err(error) => {
                    eprintln!("camera scheduling failed: {error}");
                    dispatch_after = Instant::now() + Duration::from_secs(5);
                }
            }
        }
        match listener.accept() {
            Ok((stream, _)) => {
                let timeout = Some(Duration::from_secs(5));
                let _ = stream.set_read_timeout(timeout);
                let _ = stream.set_write_timeout(timeout);
                if let Err(error) = serve_connection(&mut runtime, stream, &mut subscribers) {
                    eprintln!("worker connection error: {error}");
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(error) => eprintln!("worker accept error: {error}"),
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn serve_connection(
    runtime: &mut WorkerRuntime,
    mut stream: UnixStream,
    subscribers: &mut Vec<RevisionSubscriber>,
) -> Result<(), WorkerRunError> {
    let request: ClientRequest = read_frame(&mut stream).map_err(WorkerRunError::Frame)?;
    let subscribing = matches!(&request.command, WorkerCommand::Subscribe { .. });
    let mutating = request.command.is_mutating();
    let reply = runtime.handle(request);
    let committed_mutation = mutating && reply.ok;
    write_frame(&mut stream, &reply).map_err(WorkerRunError::Frame)?;
    if subscribing
        && reply.ok
        && let Some(snapshot) = reply.snapshot
    {
        let _ = stream.set_read_timeout(None);
        let _ = stream.set_write_timeout(Some(Duration::from_millis(250)));
        subscribers.push(RevisionSubscriber {
            stream,
            project_id: snapshot.project.id,
            project_revision: snapshot.revision,
            queue_revision: snapshot.queue.revision,
        });
    }
    if committed_mutation {
        notify_subscribers(runtime, subscribers);
    }
    Ok(())
}

struct RevisionSubscriber {
    stream: UnixStream,
    project_id: String,
    project_revision: u64,
    queue_revision: u64,
}

fn notify_subscribers(runtime: &WorkerRuntime, subscribers: &mut Vec<RevisionSubscriber>) {
    subscribers.retain_mut(|subscriber| {
        let Some(project_revision) = runtime.project_revision(Some(&subscriber.project_id)) else {
            return false;
        };
        let queue_revision = runtime.queue.revision;
        if project_revision == subscriber.project_revision
            && queue_revision == subscriber.queue_revision
        {
            return true;
        }
        let event = RevisionEvent {
            protocol_version: IPC_PROTOCOL_VERSION.to_owned(),
            project_id: subscriber.project_id.clone(),
            project_revision,
            queue_revision,
        };
        if write_frame(&mut subscriber.stream, &event).is_err() {
            return false;
        }
        subscriber.project_revision = project_revision;
        subscriber.queue_revision = queue_revision;
        true
    });
}

fn prepare_socket(path: &Path) -> Result<(), WorkerRunError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| WorkerRunError::Socket {
            path: parent.to_owned(),
            source,
        })?;
    }
    match fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(source) => {
            return Err(WorkerRunError::Socket {
                path: path.to_owned(),
                source,
            });
        }
    }
    Ok(())
}

struct SocketGuard(PathBuf);

impl Drop for SocketGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

pub struct WorkerRuntime {
    paths: AppPaths,
    queue: QueueState,
    commands: HashMap<String, CommandJournalEvent>,
    adapter_config: Option<PathBuf>,
    build_executor: BuildExecutorHandle,
    camera_cancellation: Option<ExecutorCancellation>,
}

impl WorkerRuntime {
    pub fn open(paths: AppPaths) -> Result<Self, WorkerRunError> {
        Self::open_with_adapter(paths, None)
    }

    pub fn open_with_adapter(
        paths: AppPaths,
        adapter_config: Option<PathBuf>,
    ) -> Result<Self, WorkerRunError> {
        fs::create_dir_all(&paths.runtime_dir).map_err(|source| WorkerRunError::Io {
            path: paths.runtime_dir.clone(),
            source,
        })?;
        fs::create_dir_all(&paths.projects_dir).map_err(|source| WorkerRunError::Io {
            path: paths.projects_dir.clone(),
            source,
        })?;
        let queue = match read_json_if_exists(&paths.queue_file())? {
            Some(queue) => queue,
            None => {
                let queue = QueueState::default();
                write_json_atomic(&paths.queue_file(), &queue)?;
                queue
            }
        };
        if queue.schema_version != crate::domain::PROJECT_SCHEMA_VERSION {
            return Err(WorkerRunError::UnsupportedQueueSchema(queue.schema_version));
        }
        let mut commands = HashMap::new();
        for event in read_jsonl::<CommandJournalEvent>(&paths.command_journal())? {
            commands.insert(event.command_id.clone(), event);
        }
        let build_executor = BuildExecutorHandle::spawn().map_err(WorkerRunError::BuildExecutor)?;
        let mut runtime = Self {
            paths,
            queue,
            commands,
            adapter_config,
            build_executor,
            camera_cancellation: None,
        };
        runtime.rebuild_queue_from_projects()?;
        runtime.recover_prepared_commands()?;
        runtime.recover_builds()?;
        runtime.resume_auditions()?;
        Ok(runtime)
    }

    fn rebuild_queue_from_projects(&mut self) -> Result<(), WorkerRunError> {
        let mut running = None;
        let mut pending = Vec::new();
        let entries =
            fs::read_dir(&self.paths.projects_dir).map_err(|source| WorkerRunError::Io {
                path: self.paths.projects_dir.clone(),
                source,
            })?;
        for entry in entries.filter_map(Result::ok) {
            let Some(project_id) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let Ok(store) = ProjectStore::open(&self.paths.projects_dir, &project_id) else {
                continue;
            };
            self.recover_terminal_jobs(&store)?;
            let state = store.read_state()?;
            for job_id in state
                .shots
                .values()
                .filter_map(|shot| shot.active_job_id.as_deref())
            {
                let job = store.read_job(job_id)?;
                let queue_entry = QueueEntry {
                    project_id: project_id.clone(),
                    project_path: store.root().to_owned(),
                    job_id: job.job_id,
                    priority: "normal".to_owned(),
                    resource: "gpu_exclusive".to_owned(),
                };
                match job.state {
                    JobState::Queued => pending.push(queue_entry),
                    JobState::Active | JobState::Blocked if running.is_none() => {
                        running = Some(queue_entry);
                    }
                    JobState::Active | JobState::Blocked => pending.push(queue_entry),
                    _ => {}
                }
            }
        }
        pending.sort_by(|left, right| {
            (&left.project_id, &left.job_id).cmp(&(&right.project_id, &right.job_id))
        });
        if self.queue.running == running && self.queue.pending == pending {
            return Ok(());
        }
        self.queue.running = running;
        self.queue.pending = pending;
        self.queue.revision = self
            .queue
            .revision
            .checked_add(1)
            .ok_or_else(|| WorkerRunError::Recovery("queue revision overflow".to_owned()))?;
        write_json_atomic(&self.paths.queue_file(), &self.queue)?;
        Ok(())
    }

    pub(super) fn touch_queue_revision(&mut self) -> Result<(), WorkerRunError> {
        self.queue.revision = self
            .queue
            .revision
            .checked_add(1)
            .ok_or_else(|| WorkerRunError::Recovery("queue revision overflow".to_owned()))?;
        write_json_atomic(&self.paths.queue_file(), &self.queue)?;
        Ok(())
    }

    fn recover_terminal_jobs(&self, store: &ProjectStore) -> Result<(), WorkerRunError> {
        let mut state = store.read_state()?;
        let expected_revision = state.revision;
        let mut changed = false;
        let active_jobs = state
            .shots
            .values()
            .filter_map(|shot| {
                shot.active_job_id
                    .as_ref()
                    .map(|job_id| (shot.shot_id.clone(), job_id.clone()))
            })
            .collect::<Vec<_>>();
        for (shot_id, job_id) in active_jobs {
            let job = store.read_job(&job_id)?;
            match job.state {
                JobState::Completed => {
                    let take = store.read_take_metadata(&shot_id, &job.reserved_take_id)?;
                    register_candidate(&mut state, &take, &job.shot_id, &timestamp());
                    changed = true;
                }
                JobState::Failed | JobState::Cancelled => {
                    let failure = job.attempts.last().and_then(|attempt| {
                        attempt.error_code.as_ref().map(|code| FailureRecord {
                            code: code.clone(),
                            subject: shot_id.clone(),
                            message: attempt.error_message.clone().unwrap_or_else(|| {
                                format!("job `{job_id}` ended without a detailed error")
                            }),
                            occurred_at: attempt.updated_at.clone(),
                        })
                    });
                    if let Some(shot) = state.shots.get_mut(&shot_id) {
                        shot.active_job_id = None;
                        shot.audition_target_takes = None;
                        shot.stage = if job.state == JobState::Cancelled {
                            ShotStage::Pending
                        } else {
                            ShotStage::Failed
                        };
                        if let Some(failure) = &failure
                            && !shot.fail_codes.contains(&failure.code)
                        {
                            shot.fail_codes.push(failure.code.clone());
                        }
                    }
                    if let Some(failure) = failure
                        && !state.recent_failures.iter().any(|existing| {
                            existing.code == failure.code
                                && existing.subject == failure.subject
                                && existing.occurred_at == failure.occurred_at
                        })
                    {
                        state.recent_failures.push(failure);
                    }
                    changed = true;
                }
                _ => {}
            }
        }
        if changed {
            state
                .bump_revision(timestamp())
                .map_err(StoreError::Invariant)?;
            store.save_state(&state, expected_revision)?;
        }
        Ok(())
    }

    pub fn handle(&mut self, request: ClientRequest) -> WorkerReply {
        if request.protocol_version != IPC_PROTOCOL_VERSION {
            return failure(
                &request,
                "PROTOCOL_VERSION",
                format!(
                    "client protocol is `{}`, expected `{IPC_PROTOCOL_VERSION}`",
                    request.protocol_version
                ),
                false,
                None,
            );
        }
        if Ulid::from_string(&request.command_id).is_err() {
            return failure(
                &request,
                "COMMAND_ID_INVALID",
                "command_id must be a ULID".to_owned(),
                false,
                None,
            );
        }
        if !request.command.is_mutating() {
            return self.execute(&request);
        }

        let request_hash = match sha256_json(&request) {
            Ok(hash) => hash,
            Err(error) => {
                return failure(
                    &request,
                    "COMMAND_HASH_FAILED",
                    error.to_string(),
                    false,
                    None,
                );
            }
        };
        if let Some(existing) = self.commands.get(&request.command_id) {
            if existing.request_hash != request_hash {
                return failure(
                    &request,
                    "COMMAND_ID_REUSED",
                    "command_id was already used for a different request".to_owned(),
                    false,
                    existing.reply.as_ref().and_then(|reply| reply.revision),
                );
            }
            return existing.reply.clone().unwrap_or_else(|| {
                failure(
                    &request,
                    "COMMAND_RECOVERY_REQUIRED",
                    "the worker stopped after preparing this command; automatic replay is blocked"
                        .to_owned(),
                    false,
                    None,
                )
            });
        }

        let prepared = CommandJournalEvent {
            event_id: format!("CJE-{}", Ulid::new()),
            command_id: request.command_id.clone(),
            request_hash: request_hash.clone(),
            project_id: recovery_project_id(&request),
            command_kind: command_kind(&request.command).to_owned(),
            status: CommandJournalStatus::Prepared,
            reply: None,
            occurred_at: timestamp(),
        };
        if let Err(error) = append_jsonl(&self.paths.command_journal(), &prepared) {
            return failure(
                &request,
                "COMMAND_JOURNAL_FAILED",
                error.to_string(),
                true,
                None,
            );
        }
        self.commands
            .insert(request.command_id.clone(), prepared.clone());

        let reply = self.execute(&request);
        let committed = CommandJournalEvent {
            event_id: format!("CJE-{}", Ulid::new()),
            command_id: request.command_id.clone(),
            request_hash,
            project_id: prepared.project_id.clone(),
            command_kind: prepared.command_kind.clone(),
            status: CommandJournalStatus::Committed,
            reply: Some(reply.clone()),
            occurred_at: timestamp(),
        };
        if let Err(error) = append_jsonl(&self.paths.command_journal(), &committed) {
            return failure(
                &request,
                "COMMAND_COMMIT_UNCERTAIN",
                format!("command result could not be journaled: {error}"),
                false,
                reply.revision,
            );
        }
        self.commands.insert(request.command_id.clone(), committed);
        reply
    }

    fn recover_prepared_commands(&mut self) -> Result<(), WorkerRunError> {
        let pending = self
            .commands
            .values()
            .filter(|event| event.status == CommandJournalStatus::Prepared)
            .cloned()
            .collect::<Vec<_>>();
        for prepared in pending {
            let request = ClientRequest {
                protocol_version: IPC_PROTOCOL_VERSION.to_owned(),
                command_id: prepared.command_id.clone(),
                expected_revision: None,
                project_id: prepared.project_id.clone(),
                command: WorkerCommand::Health,
            };
            let applied = self.command_was_applied(&prepared);
            let reply = if applied {
                let project_id = prepared.project_id.as_deref().ok_or_else(|| {
                    WorkerRunError::Recovery("applied command has no project id".to_owned())
                })?;
                let snapshot = self
                    .snapshot_for(Some(project_id))
                    .map_err(|error| WorkerRunError::Recovery(error.to_string()))?;
                success(
                    &request,
                    Some(snapshot.revision),
                    Some(snapshot),
                    &format!("recovered committed {} command", prepared.command_kind),
                )
            } else {
                failure(
                    &request,
                    "COMMAND_ABORTED_BEFORE_COMMIT",
                    format!(
                        "prepared {} command did not change project state",
                        prepared.command_kind
                    ),
                    true,
                    prepared
                        .project_id
                        .as_deref()
                        .and_then(|id| self.project_revision(Some(id))),
                )
            };
            let committed = CommandJournalEvent {
                event_id: format!("CJE-{}", Ulid::new()),
                command_id: prepared.command_id.clone(),
                request_hash: prepared.request_hash.clone(),
                project_id: prepared.project_id.clone(),
                command_kind: prepared.command_kind.clone(),
                status: CommandJournalStatus::Committed,
                reply: Some(reply),
                occurred_at: timestamp(),
            };
            append_jsonl(&self.paths.command_journal(), &committed)?;
            self.commands.insert(prepared.command_id.clone(), committed);
        }
        Ok(())
    }

    fn command_was_applied(&self, prepared: &CommandJournalEvent) -> bool {
        if matches!(
            prepared.command_kind.as_str(),
            "queue.pause" | "queue.resume"
        ) {
            return self.queue.last_command_id.as_deref() == Some(&prepared.command_id);
        }
        prepared.project_id.as_deref().is_some_and(|project_id| {
            ProjectStore::open(&self.paths.projects_dir, project_id)
                .and_then(|store| store.read_state())
                .is_ok_and(|state| state.last_command_id.as_deref() == Some(&prepared.command_id))
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum CommandJournalStatus {
    Prepared,
    Committed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CommandJournalEvent {
    event_id: String,
    command_id: String,
    request_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    project_id: Option<String>,
    #[serde(default)]
    command_kind: String,
    status: CommandJournalStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reply: Option<WorkerReply>,
    occurred_at: String,
}

struct GenerationGateError {
    code: &'static str,
    message: String,
    retryable: bool,
}

#[derive(Debug, Clone, Copy)]
enum TakeMutation {
    Select,
    Approve,
    Reject,
}

fn register_candidate(state: &mut ProjectState, take: &TakeMetadata, shot_id: &str, now: &str) {
    state.takes.insert(take.take_id.clone(), take.clone());
    let matching_profile_count = state
        .takes
        .values()
        .filter(|candidate| candidate.shot_id == shot_id && candidate.profile == take.profile)
        .count();
    let Some(shot) = state.shots.get_mut(shot_id) else {
        return;
    };
    shot.stage = ShotStage::CandidatesReady;
    shot.active_job_id = None;
    if !shot.take_ids.contains(&take.take_id) {
        shot.take_ids.push(take.take_id.clone());
    }
    if shot
        .audition_target_takes
        .is_some_and(|target| matching_profile_count >= usize::from(target))
    {
        shot.audition_target_takes = None;
    }
    let available_take_ids = shot
        .take_ids
        .iter()
        .filter(|take_id| !shot.rejected_take_ids.contains(take_id))
        .cloned()
        .collect::<Vec<_>>();

    let existing_approval = state.pending_approvals.iter().find(|approval| {
        approval.kind == ApprovalKind::CandidateSelection
            && approval.shot_id.as_deref() == Some(shot_id)
    });
    let approval_id = existing_approval
        .map(|approval| approval.approval_id.clone())
        .unwrap_or_else(|| format!("APR-{}", Ulid::new()));
    let created_at = existing_approval
        .map(|approval| approval.created_at.clone())
        .unwrap_or_else(|| now.to_owned());
    state.pending_approvals.retain(|approval| {
        approval.kind != ApprovalKind::CandidateSelection
            || approval.shot_id.as_deref() != Some(shot_id)
    });
    if !available_take_ids.is_empty() {
        state.pending_approvals.push(Approval {
            approval_id,
            kind: ApprovalKind::CandidateSelection,
            subject_id: None,
            shot_id: Some(shot_id.to_owned()),
            take_ids: available_take_ids,
            blocking: true,
            description: format!("Select a generated take for shot {shot_id}"),
            created_at,
        });
    }
}

impl TakeMutation {
    const fn message(self) -> &'static str {
        match self {
            Self::Select => "candidate selected",
            Self::Approve => "take approved",
            Self::Reject => "take rejected",
        }
    }
}

fn matching_attempt_mut<'a>(
    job: &'a mut JobJournal,
    request_id: &str,
) -> Result<&'a mut AttemptJournal, WorkerRunError> {
    job.attempts
        .iter_mut()
        .find(|attempt| attempt.request_id == request_id)
        .ok_or_else(|| {
            WorkerRunError::Recovery(format!(
                "job `{}` has no attempt `{request_id}`",
                job.job_id
            ))
        })
}

fn relative_project_path(root: &Path, path: &Path) -> Result<PathBuf, WorkerRunError> {
    path.strip_prefix(root).map(Path::to_owned).map_err(|_| {
        WorkerRunError::Recovery(format!(
            "output `{}` is outside project `{}`",
            path.display(),
            root.display()
        ))
    })
}

#[derive(Debug, Error)]
enum WorkerDomainError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("project id is required; available projects: {0:?}")]
    ProjectRequired(Vec<String>),
    #[error("cannot access `{path}`: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
}

#[derive(Debug, Error)]
pub enum WorkerRunError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("cannot access `{path}`: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("worker socket `{path}` failed: {source}")]
    Socket {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("worker frame failed: {0}")]
    Frame(#[source] crate::ipc::FrameError),
    #[error("unsupported queue schema `{0}`")]
    UnsupportedQueueSchema(String),
    #[error("cannot recover prepared command: {0}")]
    Recovery(String),
    #[error("cannot start camera executor: {0}")]
    Executor(#[source] std::io::Error),
    #[error("camera executor channel failed: {0}")]
    ExecutorChannel(String),
    #[error("cannot start build executor: {0}")]
    BuildExecutor(#[source] std::io::Error),
    #[error("build executor channel failed: {0}")]
    BuildExecutorChannel(String),
}

#[cfg(test)]
mod command_tests;
#[cfg(test)]
mod tests;
