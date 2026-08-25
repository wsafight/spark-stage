use super::*;

fn capability_report(available: bool) -> CapabilityReport {
    CapabilityReport {
        schema_version: "1.0".to_owned(),
        adapter: "test".to_owned(),
        endpoint: "http://127.0.0.1:8188".to_owned(),
        available,
        workflow_hash: None,
        server_version: None,
        operations: BTreeMap::new(),
        missing_nodes: Vec::new(),
        binding_errors: Vec::new(),
    }
}

fn binding(node: &str, input: &str) -> WorkflowBinding {
    WorkflowBinding {
        node: node.to_owned(),
        input: input.to_owned(),
    }
}

#[test]
fn workflow_validation_reports_sorted_missing_node_types() {
    let config = config(PathBuf::from("workflow.json"));
    let object_info = json!({"TextNode": {}});

    let (errors, missing_nodes) = validate_workflow(&workflow(), &config, Some(&object_info));

    assert!(errors.is_empty());
    assert_eq!(missing_nodes, ["SeedNode", "SizeNode", "VideoOutput"]);
}

#[test]
fn workflow_validation_reports_output_node_and_binding_errors() {
    let mut config = config(PathBuf::from("workflow.json"));
    config.output_node = "missing-output".to_owned();
    config
        .bindings
        .insert("prompt".to_owned(), binding("missing-prompt-node", "text"));
    let mut invalid_workflow = workflow();
    invalid_workflow["78"]["inputs"] = json!({});

    let (errors, missing_nodes) = validate_workflow(&invalid_workflow, &config, None);

    assert!(missing_nodes.is_empty());
    assert!(
        errors
            .iter()
            .any(|error| error.contains("output node `missing-output`"))
    );
    assert!(
        errors
            .iter()
            .any(|error| error.contains("missing node `missing-prompt-node`"))
    );
    assert!(
        errors
            .iter()
            .any(|error| error.contains("missing input `78.noise_seed`"))
    );
}

#[test]
fn operation_capabilities_distinguish_verified_and_unverified_bindings() {
    let mut config = config(PathBuf::from("workflow.json"));
    config.optional_bindings.extend([
        ("first_frame".to_owned(), binding("1", "image")),
        ("last_frame".to_owned(), binding("2", "image")),
        ("reference_video".to_owned(), binding("3", "video")),
    ]);
    config.verified_operations = vec!["t2v".to_owned(), "flf2v".to_owned()];
    let mut report = capability_report(true);

    fill_operation_capabilities(&mut report, &config);

    assert_eq!(report.operations["t2v"].status, CapabilityStatus::Verified);
    assert_eq!(
        report.operations["i2v"].status,
        CapabilityStatus::AvailableUnverified
    );
    assert_eq!(
        report.operations["flf2v"].status,
        CapabilityStatus::Verified
    );
    assert_eq!(
        report.operations["r2v"].status,
        CapabilityStatus::AvailableUnverified
    );
}

#[test]
fn operation_capabilities_report_missing_conditioning_bindings() {
    let config = config(PathBuf::from("workflow.json"));
    let mut report = capability_report(true);

    fill_operation_capabilities(&mut report, &config);

    assert_eq!(
        report.operations["t2v"].status,
        CapabilityStatus::AvailableUnverified
    );
    assert_eq!(
        report.operations["i2v"].status,
        CapabilityStatus::Unsupported
    );
    assert_eq!(
        report.operations["flf2v"].reason,
        "missing bindings: first_frame, last_frame"
    );
    assert_eq!(
        report.operations["r2v"].reason,
        "missing bindings: reference_video"
    );
}

#[test]
fn workflow_errors_make_all_operations_unavailable() {
    let config = config(PathBuf::from("workflow.json"));
    let mut report = capability_report(true);
    report.binding_errors.push("invalid binding".to_owned());

    fill_operation_capabilities(&mut report, &config);

    assert_eq!(report.operations.len(), 4);
    assert!(report.operations.values().all(|capability| {
        capability.status == CapabilityStatus::Unavailable
            && capability.reason == "workflow or node validation failed"
    }));
}

#[test]
fn output_parser_collects_supported_artifacts_in_stable_order() {
    let entry = json!({
        "outputs": {"120": {
            "videos": [{"filename": "take.mp4", "subfolder": "S01/takes"}],
            "gifs": [{"filename": "preview.gif", "subfolder": "", "type": "temp"}],
            "images": [{"filename": "first.png", "subfolder": "S01", "type": "output"}],
            "audio": [{"filename": "voice.wav", "subfolder": "S01", "type": "output"}]
        }}
    });

    let artifacts = parse_outputs(&entry, "120").unwrap();

    assert_eq!(artifacts.len(), 4);
    assert_eq!(
        artifacts
            .iter()
            .map(|artifact| artifact.filename.as_str())
            .collect::<Vec<_>>(),
        ["take.mp4", "preview.gif", "first.png", "voice.wav"]
    );
    assert_eq!(artifacts[0].kind, "output");
    assert_eq!(artifacts[1].kind, "temp");
}

#[test]
fn output_parser_rejects_empty_and_unsupported_outputs() {
    let empty = json!({"outputs": {"120": {"metadata": []}}});
    assert!(matches!(
        parse_outputs(&empty, "120"),
        Err(AdapterError::Backend(message)) if message.contains("no supported artifacts")
    ));

    let unsupported = json!({
        "outputs": {"120": {"videos": [{
            "filename": "take.mp4", "subfolder": "", "type": "private"
        }]}}
    });
    assert!(matches!(
        parse_outputs(&unsupported, "120"),
        Err(AdapterError::UnsafeOutput(message)) if message.contains("artifact type")
    ));
}

#[test]
fn request_ids_and_file_components_reject_unsafe_characters() {
    for request_id in ["", "request/1", "request_1", "请求-1"] {
        assert!(validate_request_id(request_id).is_err(), "{request_id}");
    }
    for filename in ["", ".", "..", "folder/take.mp4", "/take.mp4"] {
        assert!(validate_file_component(filename).is_err(), "{filename}");
    }
    assert!(validate_request_id("REQUEST-01ARZ3NDEKTSV4RRFFQ69G5FAV").is_ok());
    assert!(validate_file_component("take 01.mp4").is_ok());
}

#[test]
fn history_and_queue_failures_have_deterministic_fallbacks() {
    assert_eq!(
        history_state(&json!({"status": {"status_str": "failed"}})),
        BackendState::Failed {
            message: "ComfyUI history reports an execution error".to_owned()
        }
    );
    assert_eq!(
        queue_state(&json!({"queue_running": [[1, "other"]]}), "missing"),
        BackendState::NotFound
    );
    assert_eq!(
        execution_error_message(&json!({"exception_type": "RuntimeError"})),
        "RuntimeError"
    );
}
