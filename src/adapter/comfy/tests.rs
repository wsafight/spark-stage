use super::*;
use crate::adapter::CameraAdapter;

mod http;
mod protocol;

fn workflow() -> Value {
    json!({
        "45": {"class_type": "TextNode", "inputs": {"text": "old"}},
        "78": {"class_type": "SeedNode", "inputs": {"noise_seed": 1}},
        "90": {"class_type": "SizeNode", "inputs": {"width": 1, "height": 1}},
        "120": {"class_type": "VideoOutput", "inputs": {"filename_prefix": "old"}}
    })
}

pub(super) fn config(workflow: PathBuf) -> ComfyAdapterConfig {
    ComfyAdapterConfig {
        schema_version: "1.0".to_owned(),
        adapter: "minimax-h3-comfy".to_owned(),
        enabled: true,
        endpoint: "http://127.0.0.1:8188".to_owned(),
        allow_remote: false,
        allow_global_interrupt: false,
        workflow,
        output_node: "120".to_owned(),
        model_fingerprint: Some("test-model".to_owned()),
        bindings: BTreeMap::from([
            (
                "prompt".to_owned(),
                WorkflowBinding {
                    node: "45".to_owned(),
                    input: "text".to_owned(),
                },
            ),
            (
                "seed".to_owned(),
                WorkflowBinding {
                    node: "78".to_owned(),
                    input: "noise_seed".to_owned(),
                },
            ),
            (
                "output_prefix".to_owned(),
                WorkflowBinding {
                    node: "120".to_owned(),
                    input: "filename_prefix".to_owned(),
                },
            ),
            (
                "width".to_owned(),
                WorkflowBinding {
                    node: "90".to_owned(),
                    input: "width".to_owned(),
                },
            ),
            (
                "height".to_owned(),
                WorkflowBinding {
                    node: "90".to_owned(),
                    input: "height".to_owned(),
                },
            ),
        ]),
        optional_bindings: BTreeMap::new(),
        profiles: BTreeMap::from([("audition".to_owned(), BTreeMap::new())]),
        verified_operations: Vec::new(),
    }
}

#[tokio::test]
async fn prepare_changes_only_declared_bindings() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("workflow.json");
    std::fs::write(&path, serde_json::to_vec(&workflow()).unwrap()).unwrap();
    let adapter = ComfyAdapter::new(config(path)).unwrap();

    let prepared = adapter
        .prepare(GenerationRequest {
            request_id: "01ARZ3NDEKTSV4RRFFQ69G5FAV".to_owned(),
            operation: Operation::T2v,
            prompt: "rain at night".to_owned(),
            seed: 42,
            width: 960,
            height: 544,
            fps: 24,
            duration_seconds: 5,
            profile: "audition".to_owned(),
            first_frame: None,
            last_frame: None,
            reference_video: None,
        })
        .await
        .unwrap();

    assert_eq!(prepared.workflow["45"]["inputs"]["text"], "rain at night");
    assert_eq!(prepared.workflow["78"]["inputs"]["noise_seed"], 42);
    assert_eq!(prepared.workflow["90"]["inputs"]["width"], 960);
    assert_eq!(
        prepared.workflow["120"]["inputs"]["filename_prefix"],
        "sparkstage/01ARZ3NDEKTSV4RRFFQ69G5FAV"
    );
}

#[test]
fn remote_endpoint_requires_explicit_opt_in() {
    let mut config = config(PathBuf::from("workflow.json"));
    config.endpoint = "http://192.0.2.10:8188".to_owned();
    assert!(matches!(
        ComfyAdapter::new(config),
        Err(AdapterError::Endpoint(_))
    ));
}

#[test]
fn output_parser_rejects_parent_traversal() {
    let entry = json!({
        "status": {"status_str": "success", "completed": true},
        "outputs": {"120": {"videos": [{
            "filename": "../escape.mp4", "subfolder": "", "type": "output"
        }]}}
    });
    assert!(matches!(
        parse_outputs(&entry, "120"),
        Err(AdapterError::UnsafeOutput(_))
    ));
}

#[test]
fn history_and_queue_states_are_structural() {
    assert_eq!(
        history_state(&json!({"status": {"status_str": "success"}})),
        BackendState::Succeeded
    );
    assert_eq!(
        queue_state(&json!({"queue_pending": [[1, "prompt-1", {}]]}), "prompt-1"),
        BackendState::Queued
    );
}
