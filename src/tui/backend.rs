use std::path::PathBuf;

pub use crate::ipc::ClientError as BackendError;
use crate::ipc::WorkerClient;

use super::protocol::{AppSnapshot, WorkerCommand};

#[derive(Debug, Clone)]
pub struct BackendReply {
    pub snapshot: Option<AppSnapshot>,
    pub artifact_path: Option<PathBuf>,
    pub message: Option<String>,
}

pub trait TuiBackend {
    fn refresh(&mut self) -> Result<AppSnapshot, BackendError>;
    fn dispatch(
        &mut self,
        command: WorkerCommand,
        expected_revision: u64,
    ) -> Result<BackendReply, BackendError>;
}

#[derive(Debug)]
pub struct UnixBackend {
    client: WorkerClient,
}

impl UnixBackend {
    #[must_use]
    pub fn new(socket: PathBuf, project_id: Option<String>) -> Self {
        Self {
            client: WorkerClient::new(socket, project_id),
        }
    }
}

impl TuiBackend for UnixBackend {
    fn refresh(&mut self) -> Result<AppSnapshot, BackendError> {
        self.client
            .send(WorkerCommand::Snapshot, None)?
            .snapshot
            .ok_or(BackendError::MissingSnapshot)
    }

    fn dispatch(
        &mut self,
        command: WorkerCommand,
        expected_revision: u64,
    ) -> Result<BackendReply, BackendError> {
        let reply = self.client.send(command, Some(expected_revision))?;
        Ok(BackendReply {
            snapshot: reply.snapshot,
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
}
