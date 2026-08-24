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

use crate::adapter::{ComfyAdapter, ComfyAdapterConfig};
use crate::domain::{
    Approval, ApprovalKind, AttemptJournal, AttemptState, FailureRecord, JobJournal, JobState,
    Operation, ProjectOutcome, ProjectStage, ProjectState, PromotionStrategy, QualityTarget,
    QueueEntry, QueueState, Risk, ShotStage, TakeMetadata, WorkMode,
};
use crate::ipc::{
    AppSnapshot, ApprovalSummary, BudgetSummary, BuildSummary, ClientRequest, DiagnosticSummary,
    FailureSummary, GpuSummary, IPC_PROTOCOL_VERSION, ProjectSummary, QueueJobSummary,
    QueueSummary, ShotSummary, TakeSummary, WorkerCommand, WorkerError, WorkerReply, read_frame,
    write_frame,
};
use crate::paths::AppPaths;
use crate::store::{
    ExclusiveFileLock, ProjectStore, StoreError, append_jsonl, read_json_if_exists, read_jsonl,
    sha256_json, write_json_atomic,
};
use crate::validation::validate_json;

use super::executor::{ExecutionContext, ExecutorEvent, ExecutorHandle, ExecutorRequest};

mod commands;
mod execution;
mod snapshot;

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
    let mut executor_busy = false;
    let mut dispatch_after = Instant::now();

    loop {
        loop {
            match executor.try_recv() {
                Ok(event) => {
                    let finishes_request = event.finishes_request();
                    let retry_delay = event.retry_delay();
                    match runtime.apply_executor_event(event) {
                        Ok(Some(request)) => {
                            executor.send(request).map_err(|error| {
                                WorkerRunError::ExecutorChannel(error.to_string())
                            })?;
                            executor_busy = true;
                        }
                        Ok(None) => {
                            if finishes_request {
                                executor_busy = false;
                                dispatch_after = Instant::now() + retry_delay;
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
            match runtime.next_executor_request() {
                Ok(Some(request)) => {
                    executor
                        .send(request)
                        .map_err(|error| WorkerRunError::ExecutorChannel(error.to_string()))?;
                    executor_busy = true;
                }
                Ok(None) => {}
                Err(error) => {
                    eprintln!("camera scheduling failed: {error}");
                    dispatch_after = Instant::now() + Duration::from_secs(5);
                }
            }
        }
        match listener.accept() {
            Ok((mut stream, _)) => {
                let timeout = Some(Duration::from_secs(5));
                let _ = stream.set_read_timeout(timeout);
                let _ = stream.set_write_timeout(timeout);
                if let Err(error) = serve_connection(&mut runtime, &mut stream) {
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
    stream: &mut UnixStream,
) -> Result<(), WorkerRunError> {
    let request: ClientRequest = read_frame(stream).map_err(WorkerRunError::Frame)?;
    let reply = runtime.handle(request);
    write_frame(stream, &reply).map_err(WorkerRunError::Frame)
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
        let mut runtime = Self {
            paths,
            queue,
            commands,
            adapter_config,
        };
        runtime.rebuild_queue_from_projects()?;
        runtime.recover_prepared_commands()?;
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
    let Some(shot) = state.shots.get_mut(shot_id) else {
        return;
    };
    shot.stage = ShotStage::CandidatesReady;
    shot.active_job_id = None;
    if !shot.take_ids.contains(&take.take_id) {
        shot.take_ids.push(take.take_id.clone());
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
}

fn success(
    request: &ClientRequest,
    revision: Option<u64>,
    snapshot: Option<AppSnapshot>,
    message: &str,
) -> WorkerReply {
    WorkerReply {
        protocol_version: IPC_PROTOCOL_VERSION.to_owned(),
        command_id: request.command_id.clone(),
        ok: true,
        revision,
        snapshot,
        artifact_path: None,
        message: Some(message.to_owned()),
        error: None,
    }
}

fn failure(
    request: &ClientRequest,
    code: &str,
    message: String,
    retryable: bool,
    revision: Option<u64>,
) -> WorkerReply {
    WorkerReply {
        protocol_version: IPC_PROTOCOL_VERSION.to_owned(),
        command_id: request.command_id.clone(),
        ok: false,
        revision,
        snapshot: None,
        artifact_path: None,
        message: None,
        error: Some(WorkerError {
            code: code.to_owned(),
            message,
            retryable,
            current_revision: revision,
        }),
    }
}

fn missing_revision(request: &ClientRequest) -> WorkerReply {
    failure(
        request,
        "EXPECTED_REVISION_REQUIRED",
        "mutating project commands require expected_revision".to_owned(),
        false,
        None,
    )
}

fn store_failure(request: &ClientRequest, error: StoreError) -> WorkerReply {
    match error {
        StoreError::RevisionConflict { actual, .. } => failure(
            request,
            "REVISION_CONFLICT",
            "project state changed; refresh before retrying".to_owned(),
            true,
            Some(actual),
        ),
        StoreError::ProjectNotFound(_) => {
            failure(request, "PROJECT_NOT_FOUND", error.to_string(), false, None)
        }
        StoreError::ProjectExists(_) => {
            failure(request, "PROJECT_EXISTS", error.to_string(), false, None)
        }
        StoreError::InvalidProjectId(_) | StoreError::ProjectIdMismatch { .. } => {
            failure(request, "PROJECT_INVALID", error.to_string(), false, None)
        }
        StoreError::LockBusy { .. } => {
            failure(request, "RESOURCE_BUSY", error.to_string(), true, None)
        }
        StoreError::NoPendingScriptApproval | StoreError::ContractNotFound(_) => failure(
            request,
            "APPROVAL_NOT_FOUND",
            error.to_string(),
            false,
            None,
        ),
        StoreError::NoActiveContract => failure(
            request,
            "ACTIVE_CONTRACT_REQUIRED",
            error.to_string(),
            false,
            None,
        ),
        StoreError::ShotNotFound(_) => {
            failure(request, "SHOT_NOT_FOUND", error.to_string(), false, None)
        }
        StoreError::ShotBusy { .. } => {
            failure(request, "SHOT_BUSY", error.to_string(), false, None)
        }
        StoreError::InvalidJob(_) => {
            failure(request, "JOB_INVALID", error.to_string(), false, None)
        }
        StoreError::JobNotCancellable { .. } => failure(
            request,
            "JOB_NOT_CANCELLABLE",
            error.to_string(),
            false,
            None,
        ),
        StoreError::TakeNotFound(_) => {
            failure(request, "TAKE_NOT_FOUND", error.to_string(), false, None)
        }
        StoreError::TakeShotMismatch { .. } => failure(
            request,
            "TAKE_SHOT_MISMATCH",
            error.to_string(),
            false,
            None,
        ),
        StoreError::TakeStale(_) => failure(request, "TAKE_STALE", error.to_string(), false, None),
        StoreError::TakeRejected(_) => {
            failure(request, "TAKE_REJECTED", error.to_string(), false, None)
        }
        StoreError::TakeNotSelected(_) => {
            failure(request, "TAKE_NOT_SELECTED", error.to_string(), false, None)
        }
        StoreError::ShotAlreadyApproved(_) => failure(
            request,
            "SHOT_ALREADY_APPROVED",
            error.to_string(),
            false,
            None,
        ),
        StoreError::TakeUnavailable(_) => {
            failure(request, "TAKE_UNAVAILABLE", error.to_string(), false, None)
        }
        _ => failure(request, "STORE_ERROR", error.to_string(), false, None),
    }
}

fn worker_failure(request: &ClientRequest, error: WorkerDomainError) -> WorkerReply {
    match error {
        WorkerDomainError::Store(error) => store_failure(request, error),
        WorkerDomainError::ProjectRequired(projects) => failure(
            request,
            "PROJECT_REQUIRED",
            format!("select a project; available projects: {projects:?}"),
            false,
            None,
        ),
        WorkerDomainError::Io { .. } => {
            failure(request, "STORE_ERROR", error.to_string(), false, None)
        }
    }
}

fn timestamp() -> String {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}.{:03}Z", duration.as_secs(), duration.subsec_millis())
}

fn recovery_project_id(request: &ClientRequest) -> Option<String> {
    match &request.command {
        WorkerCommand::CreateProject { project_id, .. } => Some(project_id.clone()),
        _ => request.project_id.clone(),
    }
}

const fn operation_name(operation: Operation) -> &'static str {
    match operation {
        Operation::T2v => "t2v",
        Operation::I2v => "i2v",
        Operation::Flf2v => "flf2v",
        Operation::R2v => "r2v",
    }
}

const fn operation_bindings(operation: Operation) -> &'static [&'static str] {
    match operation {
        Operation::T2v => &[],
        Operation::I2v => &["first_frame"],
        Operation::Flf2v => &["first_frame", "last_frame"],
        Operation::R2v => &["reference_video"],
    }
}

const fn command_kind(command: &WorkerCommand) -> &'static str {
    match command {
        WorkerCommand::Health => "health",
        WorkerCommand::Snapshot => "snapshot",
        WorkerCommand::CreateProject { .. } => "project.create",
        WorkerCommand::ApplyScript { .. } => "script.apply",
        WorkerCommand::ApproveScript => "script.approve",
        WorkerCommand::Approve { .. } => "approval.approve",
        WorkerCommand::RetryShot { .. } => "shot.retry",
        WorkerCommand::AuditionShot { .. } => "shot.audition",
        WorkerCommand::RenderShot { .. } => "shot.render",
        WorkerCommand::SelectTake { .. } => "take.select",
        WorkerCommand::ApproveTake { .. } => "take.approve",
        WorkerCommand::RejectTake { .. } => "take.reject",
        WorkerCommand::PreviewTake { .. } => "take.preview",
        WorkerCommand::PauseQueue => "queue.pause",
        WorkerCommand::ResumeQueue => "queue.resume",
        WorkerCommand::CancelJob { .. } => "job.cancel",
        WorkerCommand::Build { .. } => "build.create",
        WorkerCommand::OpenBuild { .. } => "build.open",
        WorkerCommand::RetryProbe { .. } => "probe.retry",
        WorkerCommand::OpenLogs => "logs.open",
    }
}

const fn project_stage(value: ProjectStage) -> &'static str {
    match value {
        ProjectStage::Authoring => "authoring",
        ProjectStage::Shooting => "shooting",
        ProjectStage::Review => "review",
        ProjectStage::Build => "build",
        ProjectStage::Completed => "completed",
    }
}

const fn project_outcome(value: ProjectOutcome) -> &'static str {
    match value {
        ProjectOutcome::InProgress => "in_progress",
        ProjectOutcome::NeedsReview => "needs_review",
        ProjectOutcome::Done => "done",
        ProjectOutcome::DoneWithWarnings => "done_with_warnings",
        ProjectOutcome::Failed => "failed",
        ProjectOutcome::Cancelled => "cancelled",
    }
}

const fn work_mode(value: WorkMode) -> &'static str {
    match value {
        WorkMode::Fast => "fast",
        WorkMode::Director => "director",
    }
}

const fn quality_target(value: QualityTarget) -> &'static str {
    match value {
        QualityTarget::DraftCut => "draft_cut",
        QualityTarget::Playable => "playable",
        QualityTarget::Approved => "approved",
    }
}

const fn approval_kind(value: ApprovalKind) -> &'static str {
    match value {
        ApprovalKind::ScriptBundle => "script_bundle",
        ApprovalKind::CandidateSelection => "candidate_selection",
        ApprovalKind::BudgetOverrun => "budget_overrun",
        ApprovalKind::FinalVisualReview => "final_visual_review",
    }
}

const fn shot_stage(value: ShotStage) -> &'static str {
    match value {
        ShotStage::Pending => "pending",
        ShotStage::Queued => "queued",
        ShotStage::Generating => "generating",
        ShotStage::CandidatesReady => "candidates_ready",
        ShotStage::Selected => "selected",
        ShotStage::Approved => "approved",
        ShotStage::Failed => "failed",
    }
}

const fn risk(value: Risk) -> &'static str {
    match value {
        Risk::Low => "low",
        Risk::Medium => "medium",
        Risk::High => "high",
    }
}

#[cfg(test)]
mod tests;
