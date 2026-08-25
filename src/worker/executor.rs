use std::collections::HashSet;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::adapter::{
    BackendJobId, BackendState, CameraAdapter, ComfyAdapter, ComfyAdapterConfig, GenerationRequest,
    PreparedJob,
};
use crate::domain::{JobJournal, ShotContract};
use crate::media::{self, BoundaryFrames, MediaReport};

const BACKEND_WAIT_TIMEOUT: Duration = Duration::from_secs(2 * 60 * 60);

#[derive(Debug, Clone)]
pub(crate) struct ExecutionContext {
    pub adapter_config: PathBuf,
    pub project_root: PathBuf,
    pub job: JobJournal,
    pub shot: ShotContract,
}

#[derive(Debug, Clone)]
pub(crate) enum ExecutorRequest {
    Prepare(ExecutionContext),
    Submit {
        context: ExecutionContext,
        prepared: PreparedJob,
    },
    Reconcile {
        context: ExecutionContext,
        request_id: String,
        client_id: String,
        workflow_hash: String,
        backend_job_id: BackendJobId,
    },
}

#[derive(Debug)]
pub(crate) enum ExecutorEvent {
    Prepared {
        context: Box<ExecutionContext>,
        prepared: Box<PreparedJob>,
    },
    Submitted {
        job_id: String,
        request_id: String,
        backend_job_id: BackendJobId,
    },
    Completed {
        job_id: String,
        request_id: String,
        workflow_hash: String,
        model_fingerprint: String,
        media_path: PathBuf,
        report: MediaReport,
        boundaries: BoundaryFrames,
        elapsed_milliseconds: u64,
    },
    OutputInvalid {
        job_id: String,
        request_id: String,
        code: String,
        message: String,
        staging_path: Option<PathBuf>,
        report: Option<MediaReport>,
    },
    BackendFailed {
        job_id: String,
        request_id: String,
        message: String,
    },
    SubmissionUnknown {
        job_id: String,
        request_id: String,
        message: String,
    },
    RetryableMonitorError {
        job_id: String,
        request_id: String,
        message: String,
    },
    PreparationFailed {
        job_id: String,
        request_id: String,
        code: String,
        message: String,
    },
    Cancelled {
        job_id: String,
        request_id: String,
    },
}

impl ExecutorEvent {
    #[must_use]
    pub(crate) const fn finishes_request(&self) -> bool {
        !matches!(self, Self::Submitted { .. })
    }

    #[must_use]
    pub(crate) const fn retry_delay(&self) -> Duration {
        if matches!(self, Self::RetryableMonitorError { .. }) {
            Duration::from_secs(5)
        } else {
            Duration::ZERO
        }
    }
}

pub(crate) struct ExecutorHandle {
    requests: Sender<Box<ExecutorRequest>>,
    events: Receiver<ExecutorEvent>,
    cancellation: ExecutorCancellation,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct ExecutorCancellation {
    requested: Arc<Mutex<HashSet<String>>>,
}

impl ExecutorCancellation {
    pub(crate) fn request(&self, job_id: &str) {
        self.requested
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(job_id.to_owned());
    }

    async fn wait(&self, job_id: &str) {
        loop {
            if self
                .requested
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .remove(job_id)
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}

impl ExecutorHandle {
    pub(crate) fn spawn() -> std::io::Result<Self> {
        let (request_tx, request_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        let cancellation = ExecutorCancellation::default();
        let executor_cancellation = cancellation.clone();
        std::thread::Builder::new()
            .name("sparkstage-camera".to_owned())
            .spawn(move || executor_thread(request_rx, &event_tx, &executor_cancellation))?;
        Ok(Self {
            requests: request_tx,
            events: event_rx,
            cancellation,
        })
    }

    pub(crate) fn send(&self, request: ExecutorRequest) -> Result<(), String> {
        self.requests
            .send(Box::new(request))
            .map_err(|_| "camera executor request channel is closed".to_owned())
    }

    pub(crate) fn try_recv(&self) -> Result<ExecutorEvent, TryRecvError> {
        self.events.try_recv()
    }

    pub(crate) fn cancellation(&self) -> ExecutorCancellation {
        self.cancellation.clone()
    }
}

fn executor_thread(
    requests: Receiver<Box<ExecutorRequest>>,
    events: &Sender<ExecutorEvent>,
    cancellation: &ExecutorCancellation,
) {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("camera executor runtime failed: {error}");
            return;
        }
    };
    while let Ok(request) = requests.recv() {
        runtime.block_on(process(*request, events, cancellation));
    }
}

async fn process(
    request: ExecutorRequest,
    events: &Sender<ExecutorEvent>,
    cancellation: &ExecutorCancellation,
) {
    match request {
        ExecutorRequest::Prepare(context) => prepare(context, events).await,
        ExecutorRequest::Submit { context, prepared } => {
            submit(context, prepared, events, cancellation).await;
        }
        ExecutorRequest::Reconcile {
            context,
            request_id,
            client_id,
            workflow_hash,
            backend_job_id,
        } => {
            reconcile(
                context,
                request_id,
                client_id,
                workflow_hash,
                backend_job_id,
                events,
                cancellation,
            )
            .await;
        }
    }
}

async fn prepare(context: ExecutionContext, events: &Sender<ExecutorEvent>) {
    let request_id = context
        .job
        .attempts
        .last()
        .map(|attempt| attempt.request_id.clone())
        .unwrap_or_default();
    let adapter = match load_adapter(&context.adapter_config) {
        Ok(adapter) => adapter,
        Err((code, message)) => {
            send_event(
                events,
                ExecutorEvent::PreparationFailed {
                    job_id: context.job.job_id,
                    request_id,
                    code,
                    message,
                },
            );
            return;
        }
    };
    let request = generation_request(&context, request_id.clone());
    match adapter.prepare(request).await {
        Ok(prepared) => send_event(
            events,
            ExecutorEvent::Prepared {
                context: Box::new(context),
                prepared: Box::new(prepared),
            },
        ),
        Err(error) => send_event(
            events,
            ExecutorEvent::PreparationFailed {
                job_id: context.job.job_id,
                request_id,
                code: "WORKFLOW_PREPARE_FAILED".to_owned(),
                message: error.to_string(),
            },
        ),
    }
}

async fn submit(
    context: ExecutionContext,
    prepared: PreparedJob,
    events: &Sender<ExecutorEvent>,
    cancellation: &ExecutorCancellation,
) {
    let started = Instant::now();
    let request_id = prepared.request_id.clone();
    let adapter = match load_adapter(&context.adapter_config) {
        Ok(adapter) => adapter,
        Err((_, message)) => {
            send_event(
                events,
                ExecutorEvent::SubmissionUnknown {
                    job_id: context.job.job_id,
                    request_id,
                    message,
                },
            );
            return;
        }
    };
    let backend_job_id = match adapter.submit(&prepared).await {
        Ok(job_id) => job_id,
        Err(error) => {
            send_event(
                events,
                ExecutorEvent::SubmissionUnknown {
                    job_id: context.job.job_id,
                    request_id,
                    message: error.to_string(),
                },
            );
            return;
        }
    };
    send_event(
        events,
        ExecutorEvent::Submitted {
            job_id: context.job.job_id.clone(),
            request_id: request_id.clone(),
            backend_job_id: backend_job_id.clone(),
        },
    );
    let backend_state = tokio::select! {
        state = adapter.wait_websocket(
            &prepared.client_id,
            &backend_job_id,
            BACKEND_WAIT_TIMEOUT,
        ) => Some(state),
        () = cancellation.wait(&context.job.job_id) => None,
    };
    let Some(backend_state) = backend_state else {
        send_event(
            events,
            ExecutorEvent::Cancelled {
                job_id: context.job.job_id,
                request_id,
            },
        );
        return;
    };
    match backend_state {
        Ok(BackendState::Succeeded) => {
            collect_output(
                &adapter,
                context,
                request_id,
                prepared.workflow_hash,
                backend_job_id,
                started,
                events,
            )
            .await;
        }
        Ok(BackendState::Failed { message }) => send_event(
            events,
            ExecutorEvent::BackendFailed {
                job_id: context.job.job_id,
                request_id,
                message,
            },
        ),
        Ok(state) => send_event(
            events,
            ExecutorEvent::RetryableMonitorError {
                job_id: context.job.job_id,
                request_id,
                message: format!("backend monitor ended in non-terminal state {state:?}"),
            },
        ),
        Err(error) => send_event(
            events,
            ExecutorEvent::RetryableMonitorError {
                job_id: context.job.job_id,
                request_id,
                message: error.to_string(),
            },
        ),
    }
}

async fn reconcile(
    context: ExecutionContext,
    request_id: String,
    _client_id: String,
    workflow_hash: String,
    backend_job_id: BackendJobId,
    events: &Sender<ExecutorEvent>,
    cancellation: &ExecutorCancellation,
) {
    let started = Instant::now();
    let adapter = match load_adapter(&context.adapter_config) {
        Ok(adapter) => adapter,
        Err((_, message)) => {
            send_event(
                events,
                ExecutorEvent::RetryableMonitorError {
                    job_id: context.job.job_id,
                    request_id,
                    message,
                },
            );
            return;
        }
    };
    let backend_state = tokio::select! {
        state = adapter.reconcile(&backend_job_id) => Some(state),
        () = cancellation.wait(&context.job.job_id) => None,
    };
    let Some(backend_state) = backend_state else {
        send_event(
            events,
            ExecutorEvent::Cancelled {
                job_id: context.job.job_id,
                request_id,
            },
        );
        return;
    };
    match backend_state {
        Ok(BackendState::Succeeded) => {
            collect_output(
                &adapter,
                context,
                request_id,
                workflow_hash,
                backend_job_id,
                started,
                events,
            )
            .await;
        }
        Ok(BackendState::Failed { message }) => send_event(
            events,
            ExecutorEvent::BackendFailed {
                job_id: context.job.job_id,
                request_id,
                message,
            },
        ),
        Ok(state) => send_event(
            events,
            ExecutorEvent::RetryableMonitorError {
                job_id: context.job.job_id,
                request_id,
                message: format!("backend is not terminal: {state:?}"),
            },
        ),
        Err(error) => send_event(
            events,
            ExecutorEvent::RetryableMonitorError {
                job_id: context.job.job_id,
                request_id,
                message: error.to_string(),
            },
        ),
    }
}

async fn collect_output(
    adapter: &ComfyAdapter,
    context: ExecutionContext,
    request_id: String,
    workflow_hash: String,
    backend_job_id: BackendJobId,
    started: Instant,
    events: &Sender<ExecutorEvent>,
) {
    let artifacts = match adapter.fetch_outputs(&backend_job_id).await {
        Ok(artifacts) => artifacts,
        Err(error) => {
            output_error(
                events,
                &context,
                request_id,
                "OUTPUT_DISCOVERY_FAILED",
                error.to_string(),
                None,
                None,
            );
            return;
        }
    };
    let Some(artifact) = artifacts.into_iter().find(|artifact| {
        Path::new(&artifact.filename)
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| {
                matches!(
                    extension.to_ascii_lowercase().as_str(),
                    "mp4" | "mov" | "webm" | "mkv"
                )
            })
    }) else {
        output_error(
            events,
            &context,
            request_id,
            "VIDEO_OUTPUT_MISSING",
            "declared output node returned no supported video artifact".to_owned(),
            None,
            None,
        );
        return;
    };
    let extension = Path::new(&artifact.filename)
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("mp4")
        .to_ascii_lowercase();
    let raw_dir = context.project_root.join("raw").join(&context.job.shot_id);
    let staging_dir = raw_dir.join(".staging");
    let destination_name = format!("{}.{}", context.job.reserved_take_id, extension);
    let downloaded = match adapter
        .download_output(&artifact, &staging_dir, &destination_name)
        .await
    {
        Ok(downloaded) => downloaded,
        Err(error) => {
            output_error(
                events,
                &context,
                request_id,
                "OUTPUT_DOWNLOAD_FAILED",
                error.to_string(),
                None,
                None,
            );
            return;
        }
    };
    let report = match media::inspect(&downloaded.path, context.shot.duration, true) {
        Ok(report) => report,
        Err(error) => {
            output_error(
                events,
                &context,
                request_id,
                "MEDIA_CHECK_FAILED",
                error.to_string(),
                Some(downloaded.path),
                None,
            );
            return;
        }
    };
    if !report.valid {
        output_error(
            events,
            &context,
            request_id,
            "OUTPUT_INVALID",
            "media hard checks failed".to_owned(),
            Some(downloaded.path),
            Some(report),
        );
        return;
    }
    let final_path = raw_dir.join(&destination_name);
    if let Err(error) = std::fs::rename(&downloaded.path, &final_path) {
        output_error(
            events,
            &context,
            request_id,
            "OUTPUT_PROMOTION_FAILED",
            error.to_string(),
            Some(downloaded.path),
            Some(report),
        );
        return;
    }
    if let Err(error) = File::open(&raw_dir).and_then(|directory| directory.sync_all()) {
        output_error(
            events,
            &context,
            request_id,
            "OUTPUT_SYNC_FAILED",
            error.to_string(),
            Some(final_path),
            Some(report),
        );
        return;
    }
    let review_dir = context
        .project_root
        .join("review")
        .join(&context.job.shot_id);
    let boundaries = match media::extract_boundaries(
        &final_path,
        &review_dir,
        &context.job.reserved_take_id,
        report.duration_seconds,
    ) {
        Ok(boundaries) => boundaries,
        Err(error) => {
            output_error(
                events,
                &context,
                request_id,
                "BOUNDARY_EXTRACTION_FAILED",
                error.to_string(),
                Some(final_path),
                Some(report),
            );
            return;
        }
    };
    let model_fingerprint = adapter
        .config()
        .model_fingerprint
        .clone()
        .unwrap_or_else(|| "missing".to_owned());
    send_event(
        events,
        ExecutorEvent::Completed {
            job_id: context.job.job_id,
            request_id,
            workflow_hash,
            model_fingerprint,
            media_path: final_path,
            report,
            boundaries,
            elapsed_milliseconds: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        },
    );
}

fn output_error(
    events: &Sender<ExecutorEvent>,
    context: &ExecutionContext,
    request_id: String,
    code: &str,
    message: String,
    staging_path: Option<PathBuf>,
    report: Option<MediaReport>,
) {
    send_event(
        events,
        ExecutorEvent::OutputInvalid {
            job_id: context.job.job_id.clone(),
            request_id,
            code: code.to_owned(),
            message,
            staging_path,
            report,
        },
    );
}

fn generation_request(context: &ExecutionContext, request_id: String) -> GenerationRequest {
    let conditioning = context.shot.conditioning.as_ref();
    GenerationRequest {
        request_id,
        operation: context.job.operation,
        prompt: context.job.resolved_prompt.clone(),
        seed: context.job.seed,
        width: context.shot.width,
        height: context.shot.height,
        fps: context.shot.fps,
        duration_seconds: context.shot.duration,
        profile: context.job.profile.clone(),
        first_frame: conditioning.and_then(|value| value.first_frame.clone()),
        last_frame: conditioning.and_then(|value| value.last_frame.clone()),
        reference_video: conditioning.and_then(|value| value.reference_video.clone()),
    }
}

fn load_adapter(path: &Path) -> Result<ComfyAdapter, (String, String)> {
    let config = ComfyAdapterConfig::load(path)
        .map_err(|error| ("ADAPTER_CONFIG_INVALID".to_owned(), error.to_string()))?;
    ComfyAdapter::new(config)
        .map_err(|error| ("ADAPTER_CONFIG_INVALID".to_owned(), error.to_string()))
}

fn send_event(events: &Sender<ExecutorEvent>, event: ExecutorEvent) {
    if events.send(event).is_err() {
        eprintln!("worker stopped before a camera event could be delivered");
    }
}

#[cfg(test)]
mod tests;
