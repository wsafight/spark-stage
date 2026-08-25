mod atomic;
mod lock;
mod project;

pub use atomic::*;
pub use lock::*;
pub use project::*;

use std::path::PathBuf;

use thiserror::Error;

use crate::domain::StateInvariantError;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("invalid project id `{0}`")]
    InvalidProjectId(String),
    #[error("project `{0}` already exists")]
    ProjectExists(String),
    #[error("project `{0}` does not exist")]
    ProjectNotFound(String),
    #[error("project id `{bundle}` in bundle does not match `{project}`")]
    ProjectIdMismatch { project: String, bundle: String },
    #[error("cannot access `{path}`: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("cannot decode `{path}`: {source}")]
    Decode {
        path: PathBuf,
        source: serde_json::Error,
    },
    #[error("cannot encode JSON: {0}")]
    Encode(#[from] serde_json::Error),
    #[error("state revision changed from {expected} to {actual}")]
    RevisionConflict { expected: u64, actual: u64 },
    #[error("state invariant failed: {0}")]
    Invariant(#[from] StateInvariantError),
    #[error("lock `{path}` is already held")]
    LockBusy { path: PathBuf },
    #[error("script bundle has no pending approval")]
    NoPendingScriptApproval,
    #[error("approval `{0}` does not exist")]
    ApprovalNotFound(String),
    #[error("contract `{0}` is missing")]
    ContractNotFound(String),
    #[error("project has no active script contract")]
    NoActiveContract,
    #[error("shot `{0}` is missing from the active contract")]
    ShotNotFound(String),
    #[error("shot `{shot_id}` already has active job `{job_id}`")]
    ShotBusy { shot_id: String, job_id: String },
    #[error("build `{build_id}` is running")]
    BuildBusy { build_id: String },
    #[error("job journal is invalid: {0}")]
    InvalidJob(String),
    #[error("job `{job_id}` is `{state}` and cannot be cancelled from the pending queue")]
    JobNotCancellable { job_id: String, state: String },
    #[error("take `{0}` does not exist")]
    TakeNotFound(String),
    #[error("take `{take_id}` does not belong to shot `{shot_id}`")]
    TakeShotMismatch { take_id: String, shot_id: String },
    #[error("take `{0}` is stale")]
    TakeStale(String),
    #[error("take `{0}` was rejected")]
    TakeRejected(String),
    #[error("take `{0}` must be selected before approval")]
    TakeNotSelected(String),
    #[error("shot `{0}` already has an approved take")]
    ShotAlreadyApproved(String),
    #[error("take `{0}` cannot be selected or approved for this shot")]
    TakeUnavailable(String),
}

pub(crate) fn io_error(path: impl Into<PathBuf>, source: std::io::Error) -> StoreError {
    StoreError::Io {
        path: path.into(),
        source,
    }
}
