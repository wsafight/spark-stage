use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use futures_util::StreamExt;
use reqwest::Url;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use tokio::io::AsyncWriteExt;
use tokio_tungstenite::connect_async;
use ulid::Ulid;

use super::{
    AdapterError, BackendJobId, BackendState, CameraAdapter, CancelOutcome, CapabilityReport,
    CapabilityStatus, DownloadedArtifact, GenerationRequest, OperationCapability, OutputArtifact,
    PreparedJob,
};
use crate::domain::Operation;
use crate::media::MediaCheckPolicy;
use crate::store::sha256_json;

mod protocol;

use protocol::*;

const ADAPTER_SCHEMA_VERSION: &str = "1.0";
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Deserialize)]
struct UploadReply {
    name: String,
    #[serde(default)]
    subfolder: String,
}

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
    #[serde(default)]
    pub duration_binding_unit: DurationBindingUnit,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_fingerprint: Option<String>,
    pub bindings: BTreeMap<String, WorkflowBinding>,
    #[serde(default)]
    pub optional_bindings: BTreeMap<String, WorkflowBinding>,
    #[serde(default)]
    pub profiles: BTreeMap<String, BTreeMap<String, Value>>,
    #[serde(default)]
    pub media_check_profiles: BTreeMap<String, MediaCheckPolicy>,
    #[serde(default)]
    pub verified_operations: Vec<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DurationBindingUnit {
    #[default]
    Seconds,
    Frames,
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
        for (profile, policy) in &self.media_check_profiles {
            if !self.profiles.contains_key(profile) {
                return Err(AdapterError::Config(format!(
                    "media check profile `{profile}` has no matching generation profile"
                )));
            }
            policy.validate().map_err(|error| {
                AdapterError::Config(format!("media check profile `{profile}`: {error}"))
            })?;
        }
        Ok(())
    }

    #[must_use]
    pub fn media_check_policy(&self, profile: &str) -> MediaCheckPolicy {
        self.media_check_profiles
            .get(profile)
            .copied()
            .unwrap_or_default()
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

    async fn upload_project_file(
        &self,
        project_root: &Path,
        relative_path: &str,
    ) -> Result<String, AdapterError> {
        let root = tokio::fs::canonicalize(project_root)
            .await
            .map_err(|source| AdapterError::Io {
                path: project_root.to_owned(),
                source,
            })?;
        let relative = Path::new(relative_path);
        if relative.is_absolute()
            || relative
                .components()
                .any(|component| !matches!(component, std::path::Component::Normal(_)))
        {
            return Err(AdapterError::UnsafeOutput(format!(
                "project input `{relative_path}` is not a safe relative path"
            )));
        }
        let candidate = root.join(relative);
        let metadata = tokio::fs::symlink_metadata(&candidate)
            .await
            .map_err(|source| AdapterError::Io {
                path: candidate.clone(),
                source,
            })?;
        if !metadata.file_type().is_file() {
            return Err(AdapterError::UnsafeOutput(format!(
                "project input `{relative_path}` is not a regular file"
            )));
        }
        let canonical = tokio::fs::canonicalize(&candidate)
            .await
            .map_err(|source| AdapterError::Io {
                path: candidate.clone(),
                source,
            })?;
        if !canonical.starts_with(&root) {
            return Err(AdapterError::UnsafeOutput(format!(
                "project input `{relative_path}` escapes the project"
            )));
        }
        let filename = canonical
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or_else(|| {
                AdapterError::UnsafeOutput("project input has no safe filename".to_owned())
            })?;
        validate_file_component(filename)?;
        let part = reqwest::multipart::Part::file(&canonical)
            .await
            .map_err(|source| AdapterError::Io {
                path: canonical.clone(),
                source,
            })?
            .file_name(filename.to_owned());
        let response = self
            .client
            .post(self.endpoint_url("upload/image")?)
            .multipart(
                reqwest::multipart::Form::new()
                    .text("overwrite", "false")
                    .part("image", part),
            )
            .send()
            .await?
            .error_for_status()?
            .json::<UploadReply>()
            .await?;
        validate_file_component(&response.name)?;
        if !response.subfolder.trim().is_empty() {
            validate_relative_path(Path::new(&response.subfolder))?;
        }
        let remote = if response.subfolder.trim().is_empty() {
            response.name
        } else {
            format!("{}/{}", response.subfolder, response.name)
        };
        validate_relative_path(Path::new(&remote))?;
        Ok(remote)
    }

    async fn set_reference_images(
        &self,
        workflow: &mut Value,
        request: &GenerationRequest,
    ) -> Result<(), AdapterError> {
        const MAX_H3_REFERENCE_IMAGES: usize = 9;
        if request.reference_images.len() > MAX_H3_REFERENCE_IMAGES {
            return Err(AdapterError::Binding(format!(
                "H3 supports at most {MAX_H3_REFERENCE_IMAGES} reference images"
            )));
        }
        let binding = self.binding("reference_images").ok_or_else(|| {
            AdapterError::Binding("reference_images requires a workflow binding".to_owned())
        })?;
        let mut references = Map::new();
        let mut next_id = next_numeric_node_id(workflow);
        for (index, path) in request.reference_images.iter().enumerate() {
            let uploaded = self
                .upload_project_file(&request.project_root, path)
                .await?;
            let node_id = next_id.to_string();
            next_id = next_id.saturating_add(1);
            workflow[node_id.clone()] = json!({
                "class_type": "LoadImage",
                "inputs": {"image": uploaded}
            });
            references.insert(format!("ref_image_{index}"), json!([node_id, 0]));
        }
        set_dynamic_binding(workflow, "reference_images", binding, references)
    }

    async fn set_reference_video(
        &self,
        workflow: &mut Value,
        request: &GenerationRequest,
        relative_path: &str,
    ) -> Result<(), AdapterError> {
        let binding = self.binding("reference_video").ok_or_else(|| {
            AdapterError::Binding("reference_video requires a workflow binding".to_owned())
        })?;
        if binding.input != "ref_videos" {
            return Err(AdapterError::Binding(
                "H3 reference_video must bind to the ref_videos autogrow input".to_owned(),
            ));
        }
        let uploaded = self
            .upload_project_file(&request.project_root, relative_path)
            .await?;
        let load_video_id = next_numeric_node_id(workflow);
        workflow[load_video_id.to_string()] = json!({
            "class_type": "LoadVideo",
            "inputs": {"file": uploaded}
        });
        let components_id = load_video_id.saturating_add(1);
        workflow[components_id.to_string()] = json!({
            "class_type": "GetVideoComponents",
            "inputs": {"video": [load_video_id.to_string(), 0]}
        });
        let mut references = Map::new();
        references.insert(
            "ref_video_0".to_owned(),
            json!([components_id.to_string(), 0]),
        );
        set_dynamic_binding(workflow, "reference_video", binding, references)
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
        self.set_if_bound(
            &mut workflow,
            "prompt",
            Value::String(request.prompt.clone()),
        )?;
        self.set_if_bound(&mut workflow, "seed", Value::from(request.seed))?;
        self.set_if_bound(
            &mut workflow,
            "output_prefix",
            Value::String(output_prefix.clone()),
        )?;
        self.set_if_bound(&mut workflow, "width", Value::from(request.width))?;
        self.set_if_bound(&mut workflow, "height", Value::from(request.height))?;
        self.set_if_bound(&mut workflow, "fps", Value::from(request.fps))?;
        let duration = match self.config.duration_binding_unit {
            DurationBindingUnit::Seconds => request.duration_seconds,
            DurationBindingUnit::Frames => h3_frame_count(request.duration_seconds, request.fps),
        };
        self.set_if_bound(&mut workflow, "duration_seconds", Value::from(duration))?;
        if !request.reference_images.is_empty() {
            self.set_reference_images(&mut workflow, &request).await?;
        }
        match request.operation {
            Operation::T2v => {}
            Operation::I2v => {
                let first = request
                    .first_frame
                    .ok_or_else(|| AdapterError::Binding("i2v requires first_frame".to_owned()))?;
                let uploaded = self
                    .upload_project_file(&request.project_root, &first)
                    .await?;
                require_binding_set(self, &mut workflow, "first_frame", Value::String(uploaded))?;
            }
            Operation::Flf2v => {
                let first = request.first_frame.ok_or_else(|| {
                    AdapterError::Binding("flf2v requires first_frame".to_owned())
                })?;
                let last = request
                    .last_frame
                    .ok_or_else(|| AdapterError::Binding("flf2v requires last_frame".to_owned()))?;
                let first = self
                    .upload_project_file(&request.project_root, &first)
                    .await?;
                let last = self
                    .upload_project_file(&request.project_root, &last)
                    .await?;
                require_binding_set(self, &mut workflow, "first_frame", Value::String(first))?;
                require_binding_set(self, &mut workflow, "last_frame", Value::String(last))?;
            }
            Operation::R2v => {
                if let Some(video) = request.reference_video.clone() {
                    self.set_reference_video(&mut workflow, &request, &video)
                        .await?;
                } else if request.reference_images.is_empty() {
                    return Err(AdapterError::Binding(
                        "r2v requires reference_images or reference_video".to_owned(),
                    ));
                }
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

fn next_numeric_node_id(workflow: &Value) -> u64 {
    workflow
        .as_object()
        .into_iter()
        .flat_map(|nodes| nodes.keys())
        .filter_map(|key| key.parse::<u64>().ok())
        .max()
        .unwrap_or(0)
        .saturating_add(1)
}

fn h3_frame_count(duration_seconds: u32, fps: u32) -> u32 {
    let estimated = u64::from(duration_seconds)
        .saturating_mul(u64::from(fps))
        .max(5);
    let remainder = estimated % 17;
    let aligned = estimated.saturating_add((5 - remainder) % 17);
    aligned.min(u64::from(u32::MAX)) as u32
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

fn set_dynamic_binding(
    workflow: &mut Value,
    name: &str,
    binding: &WorkflowBinding,
    values: Map<String, Value>,
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
            "binding `{name}` refers to missing dynamic input `{}.{}`",
            binding.node, binding.input
        )));
    }
    // ComfyUI V3 autogrow inputs are submitted as dot paths, then rebuilt into
    // the nested `{ref_image_N: ...}` map immediately before node execution.
    inputs.remove(&binding.input);
    for (slot, value) in values {
        if slot.is_empty()
            || slot
                .bytes()
                .any(|byte| !(byte.is_ascii_alphanumeric() || byte == b'_'))
        {
            return Err(AdapterError::Binding(format!(
                "binding `{name}` produced an unsafe dynamic slot `{slot}`"
            )));
        }
        inputs.insert(format!("{}.{}", binding.input, slot), value);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
