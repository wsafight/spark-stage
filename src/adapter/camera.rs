use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::domain::Operation;

#[allow(async_fn_in_trait)]
pub trait CameraAdapter {
    async fn preflight(&self) -> CapabilityReport;
    async fn prepare(&self, request: GenerationRequest) -> Result<PreparedJob, AdapterError>;
    async fn submit(&self, job: &PreparedJob) -> Result<BackendJobId, AdapterError>;
    async fn reconcile(&self, job_id: &BackendJobId) -> Result<BackendState, AdapterError>;
    async fn fetch_outputs(
        &self,
        job_id: &BackendJobId,
    ) -> Result<Vec<OutputArtifact>, AdapterError>;
    async fn cancel(&self, sole_gpu_job: bool) -> Result<CancelOutcome, AdapterError>;
}

#[derive(Debug, thiserror::Error)]
pub enum AdapterError {
    #[error("cannot access adapter file `{path}`: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("cannot decode adapter YAML: {0}")]
    Yaml(#[from] serde_yaml_ng::Error),
    #[error("cannot decode workflow JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid adapter endpoint: {0}")]
    Endpoint(String),
    #[error("adapter configuration is invalid: {0}")]
    Config(String),
    #[error("workflow binding is invalid: {0}")]
    Binding(String),
    #[error("ComfyUI request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("ComfyUI backend failed: {0}")]
    Backend(String),
    #[error("unsafe output artifact: {0}")]
    UnsafeOutput(String),
    #[error("ComfyUI WebSocket failed: {0}")]
    WebSocket(String),
    #[error("ComfyUI wait timed out")]
    Timeout,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityStatus {
    Verified,
    AvailableUnverified,
    Unsupported,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OperationCapability {
    pub status: CapabilityStatus,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilityReport {
    pub schema_version: String,
    pub adapter: String,
    pub endpoint: String,
    pub available: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_version: Option<String>,
    pub operations: BTreeMap<String, OperationCapability>,
    #[serde(default)]
    pub missing_nodes: Vec<String>,
    #[serde(default)]
    pub binding_errors: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenerationRequest {
    pub request_id: String,
    pub operation: Operation,
    pub prompt: String,
    pub seed: u64,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub duration_seconds: u32,
    pub profile: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_frame: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_frame: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference_video: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedJob {
    pub request_id: String,
    pub client_id: String,
    pub workflow_hash: String,
    pub output_node: String,
    pub output_prefix: String,
    pub workflow: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct BackendJobId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum BackendState {
    Queued,
    Running {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        progress_percent: Option<u8>,
    },
    Succeeded,
    Failed {
        message: String,
    },
    NotFound,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputArtifact {
    pub node_id: String,
    pub filename: String,
    pub subfolder: String,
    pub kind: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CancelOutcome {
    Interrupted,
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DownloadedArtifact {
    pub path: PathBuf,
    pub bytes: u64,
}
