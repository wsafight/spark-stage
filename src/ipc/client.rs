use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

use thiserror::Error;
use ulid::Ulid;

use super::{
    ClientRequest, FrameError, IPC_PROTOCOL_VERSION, WorkerCommand, WorkerReply, read_frame,
    write_frame,
};

const IPC_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug)]
pub struct WorkerClient {
    socket: PathBuf,
    project_id: Option<String>,
}

impl WorkerClient {
    #[must_use]
    pub fn new(socket: PathBuf, project_id: Option<String>) -> Self {
        Self { socket, project_id }
    }

    pub fn send(
        &self,
        command: WorkerCommand,
        expected_revision: Option<u64>,
    ) -> Result<WorkerReply, ClientError> {
        let mut stream =
            UnixStream::connect(&self.socket).map_err(|source| ClientError::Connect {
                path: self.socket.clone(),
                source,
            })?;
        stream
            .set_read_timeout(Some(IPC_TIMEOUT))
            .map_err(ClientError::Io)?;
        stream
            .set_write_timeout(Some(IPC_TIMEOUT))
            .map_err(ClientError::Io)?;

        let command_id = Ulid::new().to_string();
        let request = ClientRequest {
            protocol_version: IPC_PROTOCOL_VERSION.to_owned(),
            command_id: command_id.clone(),
            expected_revision,
            project_id: self.project_id.clone(),
            command,
        };
        write_frame(&mut stream, &request).map_err(ClientError::Frame)?;
        let reply: WorkerReply = read_frame(&mut stream).map_err(ClientError::Frame)?;
        if reply.protocol_version != IPC_PROTOCOL_VERSION {
            return Err(ClientError::Protocol(format!(
                "worker protocol is `{}`, expected `{IPC_PROTOCOL_VERSION}`",
                reply.protocol_version
            )));
        }
        if reply.command_id != command_id {
            return Err(ClientError::Protocol(format!(
                "reply command id `{}` does not match request `{command_id}`",
                reply.command_id
            )));
        }
        if !reply.ok {
            let error = reply.error.unwrap_or_else(|| super::WorkerError {
                code: "WORKER_ERROR".to_owned(),
                message: reply
                    .message
                    .unwrap_or_else(|| "worker rejected command without details".to_owned()),
                retryable: false,
                current_revision: reply.revision,
            });
            return Err(ClientError::Worker {
                code: error.code,
                message: error.message,
                retryable: error.retryable,
                current_revision: error.current_revision,
            });
        }
        Ok(reply)
    }

    #[must_use]
    pub fn socket(&self) -> &std::path::Path {
        &self.socket
    }
}

#[derive(Debug, Error)]
pub enum ClientError {
    #[error("cannot connect to worker socket `{path}`: {source}")]
    Connect {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("worker IPC failed: {0}")]
    Io(#[source] std::io::Error),
    #[error("worker IPC frame failed: {0}")]
    Frame(#[source] FrameError),
    #[error("invalid worker reply: {0}")]
    Protocol(String),
    #[error("worker reply did not include a snapshot")]
    MissingSnapshot,
    #[error("{code}: {message}")]
    Worker {
        code: String,
        message: String,
        retryable: bool,
        current_revision: Option<u64>,
    },
}

impl ClientError {
    #[must_use]
    pub fn is_revision_conflict(&self) -> bool {
        matches!(self, Self::Worker { code, .. } if code == "REVISION_CONFLICT")
    }

    #[must_use]
    pub fn current_revision(&self) -> Option<u64> {
        match self {
            Self::Worker {
                current_revision, ..
            } => *current_revision,
            _ => None,
        }
    }
}
