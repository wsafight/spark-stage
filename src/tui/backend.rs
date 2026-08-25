use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError, sync_channel};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

pub use crate::ipc::ClientError as BackendError;
use crate::ipc::{
    ClientRequest, IPC_PROTOCOL_VERSION, ProjectListItem, RevisionEvent, WorkerClient,
    WorkerPayload, WorkerReply, read_frame, write_frame,
};
use ulid::Ulid;

use super::protocol::{AppSnapshot, WorkerCommand};

#[derive(Debug, Clone)]
pub struct BackendReply {
    pub snapshot: Option<AppSnapshot>,
    pub payload: Option<WorkerPayload>,
    pub artifact_path: Option<PathBuf>,
    pub message: Option<String>,
}

pub trait TuiBackend {
    fn refresh(&mut self) -> Result<AppSnapshot, BackendError>;
    fn list_projects(&mut self) -> Result<Vec<ProjectListItem>, BackendError> {
        Err(BackendError::Protocol(
            "project listing is unavailable in this backend".to_owned(),
        ))
    }
    fn select_project(&mut self, _project_id: &str) -> Result<AppSnapshot, BackendError> {
        Err(BackendError::Protocol(
            "project selection is unavailable in this backend".to_owned(),
        ))
    }
    fn selected_project_id(&self) -> Option<&str> {
        None
    }
    fn subscribe(&self) -> Option<RevisionSubscription> {
        None
    }
    fn dispatch(
        &mut self,
        command: WorkerCommand,
        expected_revision: u64,
    ) -> Result<BackendReply, BackendError>;
}

#[derive(Debug)]
pub struct UnixBackend {
    socket: PathBuf,
    client: WorkerClient,
}

impl UnixBackend {
    #[must_use]
    pub fn new(socket: PathBuf, project_id: Option<String>) -> Self {
        Self {
            client: WorkerClient::new(socket.clone(), project_id),
            socket,
        }
    }

    pub fn subscribe(&self) -> RevisionSubscription {
        RevisionSubscription::spawn(
            self.client.socket().to_owned(),
            self.client.project_id().map(str::to_owned),
        )
    }
}

pub struct RevisionSubscription {
    changes: Receiver<()>,
    stop: Arc<AtomicBool>,
    connection: Arc<Mutex<Option<UnixStream>>>,
    thread: Option<JoinHandle<()>>,
}

impl RevisionSubscription {
    fn spawn(socket: PathBuf, project_id: Option<String>) -> Self {
        let (change_tx, changes) = sync_channel(1);
        let stop = Arc::new(AtomicBool::new(false));
        let connection = Arc::new(Mutex::new(None));
        let thread_stop = stop.clone();
        let thread_connection = connection.clone();
        let thread = thread::Builder::new()
            .name("sparkstage-revisions".to_owned())
            .spawn(move || {
                revision_subscription_loop(
                    &socket,
                    project_id.as_deref(),
                    &change_tx,
                    &thread_stop,
                    &thread_connection,
                );
            })
            .ok();
        Self {
            changes,
            stop,
            connection,
            thread,
        }
    }

    pub fn changed(&self) -> bool {
        let mut changed = false;
        loop {
            match self.changes.try_recv() {
                Ok(()) => changed = true,
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => return changed,
            }
        }
    }
}

impl Drop for RevisionSubscription {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(stream) = self
            .connection
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
        {
            let _ = stream.shutdown(Shutdown::Both);
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn revision_subscription_loop(
    socket: &std::path::Path,
    project_id: Option<&str>,
    changes: &SyncSender<()>,
    stop: &AtomicBool,
    connection: &Mutex<Option<UnixStream>>,
) {
    let mut project_revision = 0;
    let mut queue_revision = 0;
    while !stop.load(Ordering::Relaxed) {
        let mut stream = match UnixStream::connect(socket) {
            Ok(stream) => stream,
            Err(_) => {
                thread::sleep(Duration::from_millis(250));
                continue;
            }
        };
        let timeout = Some(Duration::from_secs(3));
        let _ = stream.set_read_timeout(timeout);
        let _ = stream.set_write_timeout(timeout);
        if let Ok(control) = stream.try_clone() {
            *connection
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(control);
        }
        let command_id = Ulid::new().to_string();
        let request = ClientRequest {
            protocol_version: IPC_PROTOCOL_VERSION.to_owned(),
            command_id: command_id.clone(),
            expected_revision: None,
            project_id: project_id.map(str::to_owned),
            command: WorkerCommand::Subscribe {
                project_revision,
                queue_revision,
            },
        };
        if write_frame(&mut stream, &request).is_err() {
            clear_subscription_connection(connection);
            thread::sleep(Duration::from_millis(250));
            continue;
        }
        let ack: WorkerReply = match read_frame::<_, WorkerReply>(&mut stream) {
            Ok(reply)
                if reply.ok
                    && reply.protocol_version == IPC_PROTOCOL_VERSION
                    && reply.command_id == command_id =>
            {
                reply
            }
            _ => {
                clear_subscription_connection(connection);
                thread::sleep(Duration::from_millis(250));
                continue;
            }
        };
        let Some(snapshot) = ack.snapshot else {
            clear_subscription_connection(connection);
            thread::sleep(Duration::from_millis(250));
            continue;
        };
        if snapshot.revision != project_revision || snapshot.queue.revision != queue_revision {
            project_revision = snapshot.revision;
            queue_revision = snapshot.queue.revision;
            let _ = changes.try_send(());
        }
        let _ = stream.set_read_timeout(None);
        loop {
            if stop.load(Ordering::Relaxed) {
                break;
            }
            let event: RevisionEvent = match read_frame::<_, RevisionEvent>(&mut stream) {
                Ok(event) if event.protocol_version == IPC_PROTOCOL_VERSION => event,
                _ => break,
            };
            if event.project_revision != project_revision || event.queue_revision != queue_revision
            {
                project_revision = event.project_revision;
                queue_revision = event.queue_revision;
                let _ = changes.try_send(());
            }
        }
        clear_subscription_connection(connection);
        if !stop.load(Ordering::Relaxed) {
            thread::sleep(Duration::from_millis(250));
        }
    }
}

fn clear_subscription_connection(connection: &Mutex<Option<UnixStream>>) {
    connection
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take();
}

impl TuiBackend for UnixBackend {
    fn refresh(&mut self) -> Result<AppSnapshot, BackendError> {
        self.client
            .send(WorkerCommand::Snapshot, None)?
            .snapshot
            .ok_or(BackendError::MissingSnapshot)
    }

    fn list_projects(&mut self) -> Result<Vec<ProjectListItem>, BackendError> {
        let reply =
            WorkerClient::new(self.socket.clone(), None).send(WorkerCommand::ListProjects, None)?;
        match reply.payload {
            Some(WorkerPayload::ProjectList { projects }) => Ok(projects),
            _ => Err(BackendError::Protocol(
                "project list reply did not include a project list".to_owned(),
            )),
        }
    }

    fn select_project(&mut self, project_id: &str) -> Result<AppSnapshot, BackendError> {
        self.client = WorkerClient::new(self.socket.clone(), Some(project_id.to_owned()));
        self.refresh()
    }

    fn selected_project_id(&self) -> Option<&str> {
        self.client.project_id()
    }

    fn subscribe(&self) -> Option<RevisionSubscription> {
        Some(UnixBackend::subscribe(self))
    }

    fn dispatch(
        &mut self,
        command: WorkerCommand,
        expected_revision: u64,
    ) -> Result<BackendReply, BackendError> {
        let revision = command.is_mutating().then_some(expected_revision);
        let reply = self.client.send(command, revision)?;
        Ok(BackendReply {
            snapshot: reply.snapshot,
            payload: reply.payload,
            artifact_path: reply.artifact_path,
            message: reply.message,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::net::UnixListener;
    use std::thread;

    use super::*;
    use crate::ipc::{
        ClientRequest, IPC_PROTOCOL_VERSION, ProjectSummary, WorkerError, WorkerReply, read_frame,
        write_frame,
    };
    use ulid::Ulid;

    fn test_snapshot() -> AppSnapshot {
        AppSnapshot {
            schema_version: "1.0".to_owned(),
            revision: 11,
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
            queue: crate::ipc::QueueSummary {
                revision: 7,
                ..crate::ipc::QueueSummary::default()
            },
            ..AppSnapshot::default()
        }
    }

    fn spawn_worker(
        listener: UnixListener,
        reply: WorkerReply,
    ) -> thread::JoinHandle<ClientRequest> {
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let request: ClientRequest = read_frame(&mut stream).unwrap();
            let mut reply = reply;
            reply.command_id.clone_from(&request.command_id);
            write_frame(&mut stream, &reply).unwrap();
            request
        })
    }

    #[test]
    fn refresh_round_trip_uses_versioned_envelope() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("worker.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let snapshot = test_snapshot();
        let worker = spawn_worker(
            listener,
            WorkerReply {
                protocol_version: IPC_PROTOCOL_VERSION.to_owned(),
                command_id: String::new(),
                ok: true,
                revision: Some(snapshot.revision),
                snapshot: Some(snapshot.clone()),
                payload: None,
                artifact_path: None,
                message: None,
                error: None,
            },
        );

        let mut backend = UnixBackend::new(socket, Some("rain-apartment".to_owned()));
        assert_eq!(backend.refresh().unwrap(), snapshot);
        let request = worker.join().unwrap();
        assert_eq!(request.protocol_version, IPC_PROTOCOL_VERSION);
        assert_eq!(request.project_id.as_deref(), Some("rain-apartment"));
        assert_eq!(request.expected_revision, None);
        assert_eq!(request.command, WorkerCommand::Snapshot);
        assert!(Ulid::from_string(&request.command_id).is_ok());
    }

    #[test]
    fn revision_conflict_is_returned_without_replay() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("worker.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let worker = spawn_worker(
            listener,
            WorkerReply {
                protocol_version: IPC_PROTOCOL_VERSION.to_owned(),
                command_id: String::new(),
                ok: false,
                revision: Some(12),
                snapshot: None,
                payload: None,
                artifact_path: None,
                message: None,
                error: Some(WorkerError {
                    code: "REVISION_CONFLICT".to_owned(),
                    message: "project changed".to_owned(),
                    retryable: true,
                    current_revision: Some(12),
                }),
            },
        );

        let mut backend = UnixBackend::new(socket, None);
        let error = backend
            .dispatch(
                WorkerCommand::RetryShot {
                    shot_id: "S01".to_owned(),
                },
                11,
            )
            .unwrap_err();
        assert!(error.is_revision_conflict());
        assert_eq!(error.current_revision(), Some(12));
        let request = worker.join().unwrap();
        assert_eq!(request.expected_revision, Some(11));
        assert_eq!(
            request.command,
            WorkerCommand::RetryShot {
                shot_id: "S01".to_owned(),
            }
        );
    }

    #[test]
    fn read_only_payload_command_omits_expected_revision() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("worker.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let report = crate::store::StorageReport {
            project_id: "rain-apartment".to_owned(),
            total_bytes: 100,
            trash_bytes: 20,
            reclaimable_bytes: 10,
            reclaimable_files: 1,
        };
        let worker = spawn_worker(
            listener,
            WorkerReply {
                protocol_version: IPC_PROTOCOL_VERSION.to_owned(),
                command_id: String::new(),
                ok: true,
                revision: Some(11),
                snapshot: None,
                payload: Some(WorkerPayload::StorageReport(report.clone())),
                artifact_path: None,
                message: Some("loaded".to_owned()),
                error: None,
            },
        );
        let mut backend = UnixBackend::new(socket, Some("rain-apartment".to_owned()));

        let reply = backend.dispatch(WorkerCommand::StorageStatus, 11).unwrap();

        assert_eq!(reply.payload, Some(WorkerPayload::StorageReport(report)));
        let request = worker.join().unwrap();
        assert_eq!(request.expected_revision, None);
        assert_eq!(request.command, WorkerCommand::StorageStatus);
    }

    #[test]
    fn revision_subscription_handshake_wakes_the_tui() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("worker.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let snapshot = test_snapshot();
        let worker = spawn_worker(
            listener,
            WorkerReply {
                protocol_version: IPC_PROTOCOL_VERSION.to_owned(),
                command_id: String::new(),
                ok: true,
                revision: Some(snapshot.revision),
                snapshot: Some(snapshot),
                payload: None,
                artifact_path: None,
                message: Some("subscribed".to_owned()),
                error: None,
            },
        );
        let backend = UnixBackend::new(socket, Some("rain-apartment".to_owned()));
        let subscription = backend.subscribe();

        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while !subscription.changed() && std::time::Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        assert!(std::time::Instant::now() < deadline);
        drop(subscription);

        let request = worker.join().unwrap();
        assert_eq!(
            request.command,
            WorkerCommand::Subscribe {
                project_revision: 0,
                queue_revision: 0,
            }
        );
    }
}
