use std::collections::BTreeMap;

use super::*;
use crate::adapter::DurationBindingUnit;
use crate::adapter::WorkflowBinding;
use crate::domain::{
    AttemptJournal, AttemptState, Conditioning, JobState, Operation, ScriptBundle,
};

const BUNDLE: &str = include_str!("../../../skills/screenwriter/examples/valid-short-drama.json");

fn context(adapter_config: PathBuf) -> ExecutionContext {
    let bundle: ScriptBundle = serde_json::from_str(BUNDLE).unwrap();
    ExecutionContext {
        adapter_config,
        project_root: PathBuf::from("/tmp/sparkstage-executor-test"),
        job: JobJournal {
            schema_version: "1.0".to_owned(),
            job_id: "JOB-TEST".to_owned(),
            command_id: "CMD-TEST".to_owned(),
            project_id: bundle.project.id.clone(),
            contract_id: "CONTRACT-TEST".to_owned(),
            shot_id: bundle.shots[0].id.clone(),
            reserved_take_id: "TAKE-TEST".to_owned(),
            operation: Operation::T2v,
            resolved_prompt: "rain at night".to_owned(),
            seed: 42,
            profile: "audition".to_owned(),
            input_hash: "input-hash".to_owned(),
            adapter_fingerprint: "adapter-hash".to_owned(),
            smoke_test: false,
            parent_take_id: None,
            promotion_strategy: None,
            state: JobState::Active,
            attempts: vec![AttemptJournal {
                request_id: "REQUEST-TEST".to_owned(),
                client_id: None,
                workflow_hash: None,
                backend_job_id: None,
                state: AttemptState::Prepared,
                created_at: "100".to_owned(),
                updated_at: "100".to_owned(),
                error_code: None,
                error_message: None,
                output_path: None,
            }],
        },
        shot: bundle.shots[0].clone(),
    }
}

fn adapter_config(directory: &Path, broken_prompt: bool) -> PathBuf {
    let workflow = directory.join("workflow.json");
    std::fs::write(
        &workflow,
        serde_json::to_vec(&serde_json::json!({
            "1": {"class_type": "Text", "inputs": {"text": ""}},
            "2": {"class_type": "Seed", "inputs": {"seed": 0}},
            "3": {"class_type": "Output", "inputs": {"filename_prefix": "out"}}
        }))
        .unwrap(),
    )
    .unwrap();
    let config = ComfyAdapterConfig {
        schema_version: "1.0".to_owned(),
        adapter: "executor-test".to_owned(),
        enabled: true,
        endpoint: "http://127.0.0.1:8188".to_owned(),
        allow_remote: false,
        allow_global_interrupt: false,
        workflow,
        output_node: "3".to_owned(),
        duration_binding_unit: DurationBindingUnit::Seconds,
        model_fingerprint: Some("model-test".to_owned()),
        bindings: BTreeMap::from([
            (
                "prompt".to_owned(),
                WorkflowBinding {
                    node: "1".to_owned(),
                    input: if broken_prompt { "missing" } else { "text" }.to_owned(),
                },
            ),
            (
                "seed".to_owned(),
                WorkflowBinding {
                    node: "2".to_owned(),
                    input: "seed".to_owned(),
                },
            ),
            (
                "output_prefix".to_owned(),
                WorkflowBinding {
                    node: "3".to_owned(),
                    input: "filename_prefix".to_owned(),
                },
            ),
        ]),
        optional_bindings: BTreeMap::new(),
        profiles: BTreeMap::from([("audition".to_owned(), BTreeMap::new())]),
        media_check_profiles: BTreeMap::new(),
        verified_operations: Vec::new(),
    };
    let path = directory.join("adapter.yaml");
    std::fs::write(&path, serde_yaml_ng::to_string(&config).unwrap()).unwrap();
    path
}

#[tokio::test]
async fn cancellation_signal_releases_waiter() {
    let cancellation = ExecutorCancellation::default();
    cancellation.request("JOB-test");

    tokio::time::timeout(Duration::from_millis(100), cancellation.wait("JOB-test"))
        .await
        .unwrap();
}

#[test]
fn generation_request_preserves_job_and_conditioning_contract() {
    let mut context = context(PathBuf::from("adapter.yaml"));
    context.job.operation = Operation::Flf2v;
    context.shot.conditioning = Some(Conditioning {
        first_frame: Some("review/S00/last.jpg".to_owned()),
        last_frame: Some("reference/S01/target.jpg".to_owned()),
        reference_images: Vec::new(),
        reference_video: Some("reference/S01/motion.mp4".to_owned()),
    });

    let request = generation_request(&context, "REQUEST-NEW".to_owned());

    assert_eq!(request.request_id, "REQUEST-NEW");
    assert_eq!(request.operation, Operation::Flf2v);
    assert_eq!(request.prompt, "rain at night");
    assert_eq!(request.seed, 42);
    assert_eq!(request.profile, "audition");
    assert_eq!(request.width, context.shot.width);
    assert_eq!(request.height, context.shot.height);
    assert_eq!(request.fps, context.shot.fps);
    assert_eq!(request.duration_seconds, context.shot.duration);
    assert_eq!(request.first_frame.as_deref(), Some("review/S00/last.jpg"));
    assert_eq!(
        request.reference_video.as_deref(),
        Some("reference/S01/motion.mp4")
    );
}

#[tokio::test]
async fn prepare_reports_success_config_failure_and_binding_failure() {
    let directory = tempfile::tempdir().unwrap();
    let (events, received) = mpsc::channel();
    prepare(context(adapter_config(directory.path(), false)), &events).await;
    let ExecutorEvent::Prepared {
        context: prepared_context,
        prepared,
    } = received.recv().unwrap()
    else {
        panic!("valid local workflow should be prepared");
    };
    assert_eq!(prepared_context.job.job_id, "JOB-TEST");
    assert_eq!(prepared.request_id, "REQUEST-TEST");
    assert_eq!(prepared.workflow["1"]["inputs"]["text"], "rain at night");

    let (events, received) = mpsc::channel();
    prepare(context(directory.path().join("missing.yaml")), &events).await;
    let ExecutorEvent::PreparationFailed {
        code,
        request_id,
        message,
        ..
    } = received.recv().unwrap()
    else {
        panic!("missing config should fail preparation");
    };
    assert_eq!(code, "ADAPTER_CONFIG_INVALID");
    assert_eq!(request_id, "REQUEST-TEST");
    assert!(message.contains("missing.yaml"));

    let broken = directory.path().join("broken");
    std::fs::create_dir(&broken).unwrap();
    let (events, received) = mpsc::channel();
    prepare(context(adapter_config(&broken, true)), &events).await;
    let ExecutorEvent::PreparationFailed { code, message, .. } = received.recv().unwrap() else {
        panic!("invalid workflow binding should fail preparation");
    };
    assert_eq!(code, "WORKFLOW_PREPARE_FAILED");
    assert!(message.contains("missing"));
}

#[tokio::test]
async fn process_routes_invalid_submit_and_reconcile_without_network() {
    let missing = PathBuf::from("/definitely/missing/adapter.yaml");
    let cancellation = ExecutorCancellation::default();
    let prepared = PreparedJob {
        request_id: "REQUEST-SUBMIT".to_owned(),
        client_id: "CLIENT-TEST".to_owned(),
        workflow_hash: "workflow-hash".to_owned(),
        output_node: "3".to_owned(),
        output_prefix: "sparkstage/test".to_owned(),
        workflow: serde_json::json!({}),
    };
    let (events, received) = mpsc::channel();

    process(
        ExecutorRequest::Submit {
            context: context(missing.clone()),
            prepared,
        },
        &events,
        &cancellation,
    )
    .await;
    assert!(matches!(
        received.recv().unwrap(),
        ExecutorEvent::SubmissionUnknown { request_id, message, .. }
            if request_id == "REQUEST-SUBMIT" && message.contains("adapter.yaml")
    ));

    process(
        ExecutorRequest::Reconcile {
            context: context(missing),
            request_id: "REQUEST-RECONCILE".to_owned(),
            client_id: "CLIENT-TEST".to_owned(),
            workflow_hash: "workflow-hash".to_owned(),
            backend_job_id: BackendJobId("BACKEND-TEST".to_owned()),
        },
        &events,
        &cancellation,
    )
    .await;
    assert!(matches!(
        received.recv().unwrap(),
        ExecutorEvent::RetryableMonitorError { request_id, message, .. }
            if request_id == "REQUEST-RECONCILE" && message.contains("adapter.yaml")
    ));
}

#[test]
fn background_executor_delivers_terminal_prepare_event() {
    let handle = ExecutorHandle::spawn().unwrap();
    handle
        .send(ExecutorRequest::Prepare(context(PathBuf::from(
            "/definitely/missing/adapter.yaml",
        ))))
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    let event = loop {
        match handle.try_recv() {
            Ok(event) => break event,
            Err(TryRecvError::Empty) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            result => panic!("executor did not return an event: {result:?}"),
        }
    };

    assert!(matches!(
        event,
        ExecutorEvent::PreparationFailed { code, .. } if code == "ADAPTER_CONFIG_INVALID"
    ));
    handle.cancellation().request("unused-job");
}

#[test]
fn event_policy_and_output_errors_are_explicit() {
    let submitted = ExecutorEvent::Submitted {
        job_id: "JOB-TEST".to_owned(),
        request_id: "REQUEST-TEST".to_owned(),
        backend_job_id: BackendJobId("BACKEND-TEST".to_owned()),
    };
    assert!(!submitted.finishes_request());
    assert_eq!(submitted.retry_delay(), Duration::ZERO);

    let retry = ExecutorEvent::RetryableMonitorError {
        job_id: "JOB-TEST".to_owned(),
        request_id: "REQUEST-TEST".to_owned(),
        message: "temporary".to_owned(),
    };
    assert!(retry.finishes_request());
    assert_eq!(retry.retry_delay(), Duration::from_secs(5));

    let (events, received) = mpsc::channel();
    output_error(
        &events,
        &context(PathBuf::from("adapter.yaml")),
        "REQUEST-TEST".to_owned(),
        "OUTPUT_INVALID",
        "media checks failed".to_owned(),
        Some(PathBuf::from("raw/.staging/take.mp4")),
        None,
    );
    assert!(matches!(
        received.recv().unwrap(),
        ExecutorEvent::OutputInvalid {
            job_id,
            request_id,
            code,
            message,
            staging_path: Some(_),
            report: None,
        } if job_id == "JOB-TEST"
            && request_id == "REQUEST-TEST"
            && code == "OUTPUT_INVALID"
            && message == "media checks failed"
    ));
}
