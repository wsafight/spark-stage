use std::collections::{BTreeMap, HashSet};
use std::net::IpAddr;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use futures_util::StreamExt;
use reqwest::Url;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::AsyncWriteExt;
use tokio_tungstenite::connect_async;
use ulid::Ulid;

use super::{
    AdapterError, BackendJobId, BackendState, CameraAdapter, CancelOutcome, CapabilityReport,
    CapabilityStatus, DownloadedArtifact, GenerationRequest, OperationCapability, OutputArtifact,
    PreparedJob,
};
use crate::domain::Operation;
use crate::store::sha256_json;

const ADAPTER_SCHEMA_VERSION: &str = "1.0";
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComfyAdapterConfig {
    pub schema_version: String,
    pub adapter: String,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    pub endpoint: String,
    #[serde(default)]
    pub allow_remote: bool,
    #[serde(default)]
    pub allow_global_interrupt: bool,
    pub workflow: PathBuf,
    pub output_node: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_fingerprint: Option<String>,
    pub bindings: BTreeMap<String, WorkflowBinding>,
    #[serde(default)]
    pub optional_bindings: BTreeMap<String, WorkflowBinding>,
    #[serde(default)]
    pub profiles: BTreeMap<String, BTreeMap<String, Value>>,
    #[serde(default)]
    pub verified_operations: Vec<String>,
}

fn default_enabled() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowBinding {
    pub node: String,
    pub input: String,
}

impl ComfyAdapterConfig {
    pub fn load(path: &Path) -> Result<Self, AdapterError> {
        let source = std::fs::read_to_string(path).map_err(|source| AdapterError::Io {
            path: path.to_owned(),
            source,
        })?;
        let mut config: Self = serde_yaml_ng::from_str(&source)?;
        if config.workflow.is_relative()
            && let Some(parent) = path.parent()
        {
            config.workflow = parent.join(&config.workflow);
        }
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), AdapterError> {
        if self.schema_version != ADAPTER_SCHEMA_VERSION {
            return Err(AdapterError::Config(format!(
                "unsupported schema `{}`",
                self.schema_version
            )));
        }
        if self.adapter.trim().is_empty() || self.output_node.trim().is_empty() {
            return Err(AdapterError::Config(
                "adapter and output_node must not be empty".to_owned(),
            ));
        }
        if self.enabled
            && self
                .model_fingerprint
                .as_deref()
                .is_none_or(|fingerprint| fingerprint.trim().is_empty())
        {
            return Err(AdapterError::Config(
                "enabled adapters require model_fingerprint".to_owned(),
            ));
        }
        for required in ["prompt", "seed", "output_prefix"] {
            if !self.bindings.contains_key(required) {
                return Err(AdapterError::Config(format!(
                    "required binding `{required}` is missing"
                )));
            }
        }
        let endpoint = Url::parse(&self.endpoint)
            .map_err(|error| AdapterError::Endpoint(error.to_string()))?;
        if !matches!(endpoint.scheme(), "http" | "https") {
            return Err(AdapterError::Endpoint(
                "only http and https endpoints are supported".to_owned(),
            ));
        }
        if !self.allow_remote && !is_loopback(&endpoint) {
            return Err(AdapterError::Endpoint(
                "remote ComfyUI endpoints require allow_remote: true".to_owned(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct ComfyAdapter {
    config: ComfyAdapterConfig,
    endpoint: Url,
    client: reqwest::Client,
}

impl ComfyAdapter {
    pub fn new(config: ComfyAdapterConfig) -> Result<Self, AdapterError> {
        config.validate()?;
        let endpoint = Url::parse(&config.endpoint)
            .map_err(|error| AdapterError::Endpoint(error.to_string()))?;
        let client = reqwest::Client::builder()
            .connect_timeout(DEFAULT_TIMEOUT)
            .timeout(Duration::from_secs(30))
            .build()?;
        Ok(Self {
            config,
            endpoint,
            client,
        })
    }

    #[must_use]
    pub fn config(&self) -> &ComfyAdapterConfig {
        &self.config
    }

    pub fn validate_local_workflow(&self) -> Result<String, AdapterError> {
        let workflow = self.load_workflow()?;
        let (errors, _) = validate_workflow(&workflow, &self.config, None);
        if !errors.is_empty() {
            return Err(AdapterError::Binding(errors.join("; ")));
        }
        sha256_json(&workflow).map_err(|error| AdapterError::Config(error.to_string()))
    }

    pub async fn wait_websocket(
        &self,
        client_id: &str,
        job_id: &BackendJobId,
        timeout: Duration,
    ) -> Result<BackendState, AdapterError> {
        let mut websocket_url = self.endpoint.clone();
        websocket_url
            .set_scheme(if self.endpoint.scheme() == "https" {
                "wss"
            } else {
                "ws"
            })
            .map_err(|()| AdapterError::Endpoint("cannot form WebSocket URL".to_owned()))?;
        websocket_url.set_path("/ws");
        websocket_url.set_query(None);
        websocket_url
            .query_pairs_mut()
            .append_pair("clientId", client_id);

        let wait = async {
            let (mut stream, _) = connect_async(websocket_url.as_str())
                .await
                .map_err(|error| AdapterError::WebSocket(error.to_string()))?;
            while let Some(message) = stream.next().await {
                let message =
                    message.map_err(|error| AdapterError::WebSocket(error.to_string()))?;
                if !message.is_text() {
                    continue;
                }
                let event: Value = serde_json::from_str(
                    message
                        .to_text()
                        .map_err(|error| AdapterError::WebSocket(error.to_string()))?,
                )?;
                let event_type = event.get("type").and_then(Value::as_str);
                let data = event.get("data").unwrap_or(&Value::Null);
                let prompt_id = data.get("prompt_id").and_then(Value::as_str);
                if prompt_id != Some(job_id.0.as_str()) {
                    continue;
                }
                if event_type == Some("execution_error") {
                    return Ok(BackendState::Failed {
                        message: execution_error_message(data),
                    });
                }
                if event_type == Some("executing") && data.get("node").is_some_and(Value::is_null) {
                    return self.reconcile(job_id).await;
                }
            }
            let reconciled = self.reconcile(job_id).await?;
            if matches!(
                reconciled,
                BackendState::Succeeded | BackendState::Failed { .. }
            ) {
                Ok(reconciled)
            } else {
                Err(AdapterError::WebSocket(
                    "connection closed before a terminal event".to_owned(),
                ))
            }
        };
        tokio::time::timeout(timeout, wait)
            .await
            .map_err(|_| AdapterError::Timeout)?
    }

    pub async fn download_output(
        &self,
        artifact: &OutputArtifact,
        staging_dir: &Path,
        destination_name: &str,
    ) -> Result<DownloadedArtifact, AdapterError> {
        validate_artifact(artifact)?;
        validate_file_component(destination_name)?;
        if let Ok(metadata) = tokio::fs::symlink_metadata(staging_dir).await
            && metadata.file_type().is_symlink()
        {
            return Err(AdapterError::UnsafeOutput(
                "staging directory must not be a symlink".to_owned(),
            ));
        }
        tokio::fs::create_dir_all(staging_dir)
            .await
            .map_err(|source| AdapterError::Io {
                path: staging_dir.to_owned(),
                source,
            })?;
        let destination = staging_dir.join(destination_name);
        let temporary = staging_dir.join(format!(".download-{}.tmp", Ulid::new()));
        let url = self.endpoint_url("view")?;
        let response = self
            .client
            .get(url)
            .query(&[
                ("filename", artifact.filename.as_str()),
                ("subfolder", artifact.subfolder.as_str()),
                ("type", artifact.kind.as_str()),
            ])
            .send()
            .await?
            .error_for_status()?;
        let mut file = tokio::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)
            .await
            .map_err(|source| AdapterError::Io {
                path: temporary.clone(),
                source,
            })?;
        let mut bytes = 0_u64;
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            file.write_all(&chunk)
                .await
                .map_err(|source| AdapterError::Io {
                    path: temporary.clone(),
                    source,
                })?;
            bytes = bytes.saturating_add(chunk.len() as u64);
        }
        file.sync_all().await.map_err(|source| AdapterError::Io {
            path: temporary.clone(),
            source,
        })?;
        drop(file);
        if let Err(source) = tokio::fs::rename(&temporary, &destination).await {
            let _ = tokio::fs::remove_file(&temporary).await;
            return Err(AdapterError::Io {
                path: destination,
                source,
            });
        }
        Ok(DownloadedArtifact {
            path: destination,
            bytes,
        })
    }

    fn load_workflow(&self) -> Result<Value, AdapterError> {
        let source =
            std::fs::read_to_string(&self.config.workflow).map_err(|source| AdapterError::Io {
                path: self.config.workflow.clone(),
                source,
            })?;
        let workflow: Value = serde_json::from_str(&source)?;
        if !workflow.is_object() {
            return Err(AdapterError::Config(
                "API workflow root must be an object".to_owned(),
            ));
        }
        Ok(workflow)
    }

    fn endpoint_url(&self, path: &str) -> Result<Url, AdapterError> {
        self.endpoint
            .join(path)
            .map_err(|error| AdapterError::Endpoint(error.to_string()))
    }

    fn binding(&self, name: &str) -> Option<&WorkflowBinding> {
        self.config
            .bindings
            .get(name)
            .or_else(|| self.config.optional_bindings.get(name))
    }

    fn set_if_bound(
        &self,
        workflow: &mut Value,
        name: &str,
        value: Value,
    ) -> Result<bool, AdapterError> {
        let Some(binding) = self.binding(name) else {
            return Ok(false);
        };
        set_binding(workflow, name, binding, value)?;
        Ok(true)
    }

    fn apply_profile(&self, workflow: &mut Value, profile: &str) -> Result<(), AdapterError> {
        let Some(overrides) = self.config.profiles.get(profile) else {
            return Err(AdapterError::Config(format!(
                "profile `{profile}` is not configured"
            )));
        };
        for (name, value) in overrides {
            let binding = self.binding(name).ok_or_else(|| {
                AdapterError::Binding(format!(
                    "profile `{profile}` refers to unknown binding `{name}`"
                ))
            })?;
            set_binding(workflow, name, binding, value.clone())?;
        }
        Ok(())
    }
}

impl CameraAdapter for ComfyAdapter {
    async fn preflight(&self) -> CapabilityReport {
        let mut report = CapabilityReport {
            schema_version: ADAPTER_SCHEMA_VERSION.to_owned(),
            adapter: self.config.adapter.clone(),
            endpoint: self.endpoint.to_string(),
            available: false,
            workflow_hash: None,
            server_version: None,
            operations: BTreeMap::new(),
            missing_nodes: Vec::new(),
            binding_errors: Vec::new(),
        };
        if !self.config.enabled {
            fill_unavailable_operations(&mut report, "adapter is disabled");
            return report;
        }
        let workflow = match self.load_workflow() {
            Ok(workflow) => workflow,
            Err(error) => {
                report.binding_errors.push(error.to_string());
                fill_unavailable_operations(&mut report, "workflow is unavailable");
                return report;
            }
        };
        report.workflow_hash = sha256_json(&workflow).ok();

        let object_info_url = match self.endpoint_url("object_info") {
            Ok(url) => url,
            Err(error) => {
                report.binding_errors.push(error.to_string());
                fill_unavailable_operations(&mut report, "invalid ComfyUI endpoint");
                return report;
            }
        };
        let object_info = match self.client.get(object_info_url).send().await {
            Ok(response) => match response.error_for_status() {
                Ok(response) => match response.json::<Value>().await {
                    Ok(value) => value,
                    Err(error) => {
                        report.binding_errors.push(error.to_string());
                        fill_unavailable_operations(&mut report, "invalid /object_info reply");
                        return report;
                    }
                },
                Err(error) => {
                    report.binding_errors.push(error.to_string());
                    fill_unavailable_operations(&mut report, "ComfyUI is unavailable");
                    return report;
                }
            },
            Err(error) => {
                report.binding_errors.push(error.to_string());
                fill_unavailable_operations(&mut report, "ComfyUI is unavailable");
                return report;
            }
        };
        report.available = true;
        if let Ok(url) = self.endpoint_url("system_stats")
            && let Ok(response) = self.client.get(url).send().await
            && let Ok(value) = response.json::<Value>().await
        {
            report.server_version = value
                .pointer("/system/comfyui_version")
                .and_then(Value::as_str)
                .map(str::to_owned);
        }
        let (binding_errors, missing_nodes) =
            validate_workflow(&workflow, &self.config, Some(&object_info));
        report.binding_errors = binding_errors;
        report.missing_nodes = missing_nodes;
        fill_operation_capabilities(&mut report, &self.config);
        report
    }

    async fn prepare(&self, request: GenerationRequest) -> Result<PreparedJob, AdapterError> {
        if !self.config.enabled {
            return Err(AdapterError::Config("adapter is disabled".to_owned()));
        }
        validate_request_id(&request.request_id)?;
        let mut workflow = self.load_workflow()?;
        let (errors, _) = validate_workflow(&workflow, &self.config, None);
        if !errors.is_empty() {
            return Err(AdapterError::Binding(errors.join("; ")));
        }
        let output_prefix = format!("sparkstage/{}", request.request_id);
        self.set_if_bound(&mut workflow, "prompt", Value::String(request.prompt))?;
        self.set_if_bound(&mut workflow, "seed", Value::from(request.seed))?;
        self.set_if_bound(
            &mut workflow,
            "output_prefix",
            Value::String(output_prefix.clone()),
        )?;
        self.set_if_bound(&mut workflow, "width", Value::from(request.width))?;
        self.set_if_bound(&mut workflow, "height", Value::from(request.height))?;
        self.set_if_bound(&mut workflow, "fps", Value::from(request.fps))?;
        self.set_if_bound(
            &mut workflow,
            "duration_seconds",
            Value::from(request.duration_seconds),
        )?;
        match request.operation {
            Operation::T2v => {}
            Operation::I2v => {
                let first = request
                    .first_frame
                    .ok_or_else(|| AdapterError::Binding("i2v requires first_frame".to_owned()))?;
                require_binding_set(self, &mut workflow, "first_frame", Value::String(first))?;
            }
            Operation::Flf2v => {
                let first = request.first_frame.ok_or_else(|| {
                    AdapterError::Binding("flf2v requires first_frame".to_owned())
                })?;
                let last = request
                    .last_frame
                    .ok_or_else(|| AdapterError::Binding("flf2v requires last_frame".to_owned()))?;
                require_binding_set(self, &mut workflow, "first_frame", Value::String(first))?;
                require_binding_set(self, &mut workflow, "last_frame", Value::String(last))?;
            }
            Operation::R2v => {
                let video = request.reference_video.ok_or_else(|| {
                    AdapterError::Binding("r2v requires reference_video".to_owned())
                })?;
                require_binding_set(self, &mut workflow, "reference_video", Value::String(video))?;
            }
        }
        self.apply_profile(&mut workflow, &request.profile)?;
        Ok(PreparedJob {
            request_id: request.request_id,
            client_id: Ulid::new().to_string(),
            workflow_hash: sha256_json(&workflow)
                .map_err(|error| AdapterError::Config(error.to_string()))?,
            output_node: self.config.output_node.clone(),
            output_prefix,
            workflow,
        })
    }

    async fn submit(&self, job: &PreparedJob) -> Result<BackendJobId, AdapterError> {
        #[derive(Deserialize)]
        struct SubmitReply {
            prompt_id: String,
        }
        let response = self
            .client
            .post(self.endpoint_url("prompt")?)
            .json(&json!({
                "prompt": job.workflow,
                "client_id": job.client_id,
                "extra_data": {"sparkstage_request_id": job.request_id}
            }))
            .send()
            .await?
            .error_for_status()?
            .json::<SubmitReply>()
            .await?;
        if response.prompt_id.trim().is_empty() {
            return Err(AdapterError::Backend(
                "ComfyUI returned an empty prompt_id".to_owned(),
            ));
        }
        Ok(BackendJobId(response.prompt_id))
    }

    async fn reconcile(&self, job_id: &BackendJobId) -> Result<BackendState, AdapterError> {
        let history = self
            .client
            .get(self.endpoint_url(&format!("history/{}", job_id.0))?)
            .send()
            .await?
            .error_for_status()?
            .json::<Value>()
            .await?;
        if let Some(entry) = history.get(&job_id.0) {
            return Ok(history_state(entry));
        }
        let queue = self
            .client
            .get(self.endpoint_url("queue")?)
            .send()
            .await?
            .error_for_status()?
            .json::<Value>()
            .await?;
        Ok(queue_state(&queue, &job_id.0))
    }

    async fn fetch_outputs(
        &self,
        job_id: &BackendJobId,
    ) -> Result<Vec<OutputArtifact>, AdapterError> {
        let history = self
            .client
            .get(self.endpoint_url(&format!("history/{}", job_id.0))?)
            .send()
            .await?
            .error_for_status()?
            .json::<Value>()
            .await?;
        let entry = history
            .get(&job_id.0)
            .ok_or_else(|| AdapterError::Backend("history entry is missing".to_owned()))?;
        if !matches!(history_state(entry), BackendState::Succeeded) {
            return Err(AdapterError::Backend(
                "job is not in a successful terminal state".to_owned(),
            ));
        }
        parse_outputs(entry, &self.config.output_node)
    }

    async fn cancel(&self, sole_gpu_job: bool) -> Result<CancelOutcome, AdapterError> {
        if !self.config.allow_global_interrupt || !sole_gpu_job {
            return Ok(CancelOutcome::Unsupported);
        }
        self.client
            .post(self.endpoint_url("interrupt")?)
            .send()
            .await?
            .error_for_status()?;
        Ok(CancelOutcome::Interrupted)
    }
}

fn require_binding_set(
    adapter: &ComfyAdapter,
    workflow: &mut Value,
    name: &str,
    value: Value,
) -> Result<(), AdapterError> {
    if adapter.set_if_bound(workflow, name, value)? {
        Ok(())
    } else {
        Err(AdapterError::Binding(format!(
            "operation requires binding `{name}`"
        )))
    }
}

fn set_binding(
    workflow: &mut Value,
    name: &str,
    binding: &WorkflowBinding,
    value: Value,
) -> Result<(), AdapterError> {
    let node = workflow.get_mut(&binding.node).ok_or_else(|| {
        AdapterError::Binding(format!(
            "binding `{name}` refers to missing node `{}`",
            binding.node
        ))
    })?;
    let inputs = node
        .get_mut("inputs")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| {
            AdapterError::Binding(format!("node `{}` has no inputs object", binding.node))
        })?;
    if !inputs.contains_key(&binding.input) {
        return Err(AdapterError::Binding(format!(
            "binding `{name}` refers to missing input `{}.{}`",
            binding.node, binding.input
        )));
    }
    inputs.insert(binding.input.clone(), value);
    Ok(())
}

fn validate_workflow(
    workflow: &Value,
    config: &ComfyAdapterConfig,
    object_info: Option<&Value>,
) -> (Vec<String>, Vec<String>) {
    let mut errors = Vec::new();
    let mut missing_nodes = HashSet::new();
    let Some(nodes) = workflow.as_object() else {
        return (
            vec!["workflow root is not an object".to_owned()],
            Vec::new(),
        );
    };
    if !nodes.contains_key(&config.output_node) {
        errors.push(format!(
            "output node `{}` does not exist",
            config.output_node
        ));
    }
    for (name, binding) in config
        .bindings
        .iter()
        .chain(config.optional_bindings.iter())
    {
        let Some(node) = nodes.get(&binding.node) else {
            errors.push(format!(
                "binding `{name}` refers to missing node `{}`",
                binding.node
            ));
            continue;
        };
        if !node
            .get("inputs")
            .and_then(Value::as_object)
            .is_some_and(|inputs| inputs.contains_key(&binding.input))
        {
            errors.push(format!(
                "binding `{name}` refers to missing input `{}.{}`",
                binding.node, binding.input
            ));
        }
    }
    if let Some(object_info) = object_info {
        for node in nodes.values() {
            let Some(class_type) = node.get("class_type").and_then(Value::as_str) else {
                errors.push("workflow node has no class_type".to_owned());
                continue;
            };
            if object_info.get(class_type).is_none() {
                missing_nodes.insert(class_type.to_owned());
            }
        }
    }
    let mut missing_nodes = missing_nodes.into_iter().collect::<Vec<_>>();
    missing_nodes.sort();
    (errors, missing_nodes)
}

fn fill_operation_capabilities(report: &mut CapabilityReport, config: &ComfyAdapterConfig) {
    for (name, required) in [
        ("t2v", Vec::<&str>::new()),
        ("i2v", vec!["first_frame"]),
        ("flf2v", vec!["first_frame", "last_frame"]),
        ("r2v", vec!["reference_video"]),
    ] {
        let missing = required
            .into_iter()
            .filter(|binding| !config.optional_bindings.contains_key(*binding))
            .collect::<Vec<_>>();
        let (status, reason) = if !report.available {
            (
                CapabilityStatus::Unavailable,
                "ComfyUI is unavailable".to_owned(),
            )
        } else if !report.binding_errors.is_empty() || !report.missing_nodes.is_empty() {
            (
                CapabilityStatus::Unavailable,
                "workflow or node validation failed".to_owned(),
            )
        } else if !missing.is_empty() {
            (
                CapabilityStatus::Unsupported,
                format!("missing bindings: {}", missing.join(", ")),
            )
        } else if config
            .verified_operations
            .iter()
            .any(|operation| operation == name)
        {
            (
                CapabilityStatus::Verified,
                "recorded as smoke-tested for this workflow".to_owned(),
            )
        } else {
            (
                CapabilityStatus::AvailableUnverified,
                "bindings exist but no smoke test is recorded".to_owned(),
            )
        };
        report
            .operations
            .insert(name.to_owned(), OperationCapability { status, reason });
    }
}

fn fill_unavailable_operations(report: &mut CapabilityReport, reason: &str) {
    for name in ["t2v", "i2v", "flf2v", "r2v"] {
        report.operations.insert(
            name.to_owned(),
            OperationCapability {
                status: CapabilityStatus::Unavailable,
                reason: reason.to_owned(),
            },
        );
    }
}

fn history_state(entry: &Value) -> BackendState {
    let status = entry.pointer("/status/status_str").and_then(Value::as_str);
    let completed = entry
        .pointer("/status/completed")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if matches!(status, Some("error" | "failed")) {
        return BackendState::Failed {
            message: history_error_message(entry),
        };
    }
    if completed || status == Some("success") {
        return BackendState::Succeeded;
    }
    BackendState::Running {
        progress_percent: None,
    }
}

fn queue_state(queue: &Value, prompt_id: &str) -> BackendState {
    if queue_contains(queue.get("queue_running"), prompt_id) {
        BackendState::Running {
            progress_percent: None,
        }
    } else if queue_contains(queue.get("queue_pending"), prompt_id) {
        BackendState::Queued
    } else {
        BackendState::NotFound
    }
}

fn queue_contains(queue: Option<&Value>, prompt_id: &str) -> bool {
    queue.and_then(Value::as_array).is_some_and(|items| {
        items
            .iter()
            .any(|item| value_contains_string(item, prompt_id))
    })
}

fn value_contains_string(value: &Value, expected: &str) -> bool {
    match value {
        Value::String(value) => value == expected,
        Value::Array(values) => values
            .iter()
            .any(|value| value_contains_string(value, expected)),
        Value::Object(values) => values
            .values()
            .any(|value| value_contains_string(value, expected)),
        _ => false,
    }
}

fn parse_outputs(entry: &Value, output_node: &str) -> Result<Vec<OutputArtifact>, AdapterError> {
    let output = entry
        .get("outputs")
        .and_then(|outputs| outputs.get(output_node))
        .and_then(Value::as_object)
        .ok_or_else(|| {
            AdapterError::Backend(format!("output node `{output_node}` has no artifacts"))
        })?;
    let mut artifacts = Vec::new();
    for key in ["videos", "gifs", "images", "audio"] {
        let Some(items) = output.get(key).and_then(Value::as_array) else {
            continue;
        };
        for item in items {
            let filename = item
                .get("filename")
                .and_then(Value::as_str)
                .ok_or_else(|| AdapterError::Backend("output filename is missing".to_owned()))?;
            let subfolder = item
                .get("subfolder")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let kind = item.get("type").and_then(Value::as_str).unwrap_or("output");
            let artifact = OutputArtifact {
                node_id: output_node.to_owned(),
                filename: filename.to_owned(),
                subfolder: subfolder.to_owned(),
                kind: kind.to_owned(),
            };
            validate_artifact(&artifact)?;
            artifacts.push(artifact);
        }
    }
    if artifacts.is_empty() {
        return Err(AdapterError::Backend(format!(
            "output node `{output_node}` returned no supported artifacts"
        )));
    }
    Ok(artifacts)
}

fn validate_artifact(artifact: &OutputArtifact) -> Result<(), AdapterError> {
    validate_file_component(&artifact.filename)?;
    if !artifact.subfolder.is_empty() {
        validate_relative_path(Path::new(&artifact.subfolder))?;
    }
    if !matches!(artifact.kind.as_str(), "output" | "temp") {
        return Err(AdapterError::UnsafeOutput(format!(
            "unsupported artifact type `{}`",
            artifact.kind
        )));
    }
    Ok(())
}

fn validate_file_component(value: &str) -> Result<(), AdapterError> {
    let path = Path::new(value);
    if value.is_empty()
        || path.components().count() != 1
        || !matches!(path.components().next(), Some(Component::Normal(_)))
    {
        return Err(AdapterError::UnsafeOutput(format!(
            "`{value}` is not a safe filename"
        )));
    }
    Ok(())
}

fn validate_relative_path(path: &Path) -> Result<(), AdapterError> {
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(AdapterError::UnsafeOutput(format!(
            "`{}` is not a safe relative path",
            path.display()
        )));
    }
    Ok(())
}

fn validate_request_id(value: &str) -> Result<(), AdapterError> {
    if value.is_empty()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err(AdapterError::Config(
            "request_id must contain only ASCII letters, digits, and hyphens".to_owned(),
        ));
    }
    Ok(())
}

fn is_loopback(url: &Url) -> bool {
    url.host_str().is_some_and(|host| {
        host.eq_ignore_ascii_case("localhost")
            || host
                .parse::<IpAddr>()
                .is_ok_and(|address| address.is_loopback())
    })
}

fn history_error_message(entry: &Value) -> String {
    entry
        .pointer("/status/messages")
        .and_then(Value::as_array)
        .and_then(|messages| messages.last())
        .map(Value::to_string)
        .unwrap_or_else(|| "ComfyUI history reports an execution error".to_owned())
}

fn execution_error_message(data: &Value) -> String {
    data.get("exception_message")
        .and_then(Value::as_str)
        .or_else(|| data.get("exception_type").and_then(Value::as_str))
        .unwrap_or("ComfyUI execution failed")
        .to_owned()
}

#[cfg(test)]
mod tests;
