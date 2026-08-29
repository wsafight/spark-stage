use std::collections::HashSet;
use std::net::IpAddr;
use std::path::{Component, Path};

use super::*;

pub(super) fn validate_workflow(
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
            let Some(node_info) = object_info.get(class_type) else {
                missing_nodes.insert(class_type.to_owned());
                continue;
            };
            if node_info.get("input").is_some() {
                for input in node
                    .get("inputs")
                    .and_then(Value::as_object)
                    .into_iter()
                    .flat_map(|inputs| inputs.keys())
                {
                    if !object_info_input_exists(node_info, input) {
                        errors.push(format!(
                            "workflow node `{class_type}` has input `{input}` not reported by ComfyUI"
                        ));
                    }
                }
            }
        }
        validate_dynamic_bindings(config, nodes, object_info, &mut errors, &mut missing_nodes);
    }
    let mut missing_nodes = missing_nodes.into_iter().collect::<Vec<_>>();
    missing_nodes.sort();
    (errors, missing_nodes)
}

fn object_info_input_exists(node_info: &Value, input: &str) -> bool {
    ["required", "optional", "hidden"].into_iter().any(|group| {
        node_info
            .pointer(&format!("/input/{group}"))
            .and_then(Value::as_object)
            .is_some_and(|inputs| inputs.contains_key(input))
    })
}

fn object_info_input<'a>(node_info: &'a Value, input: &str) -> Option<&'a Value> {
    ["required", "optional"].into_iter().find_map(|group| {
        node_info
            .pointer(&format!("/input/{group}"))
            .and_then(|inputs| inputs.get(input))
    })
}

fn validate_dynamic_bindings(
    config: &ComfyAdapterConfig,
    workflow: &Map<String, Value>,
    object_info: &Value,
    errors: &mut Vec<String>,
    missing_nodes: &mut HashSet<String>,
) {
    for (name, expected_input, prefix, slot, helpers) in [
        (
            "reference_images",
            "ref_images",
            "ref_image_",
            "ref_image",
            &["LoadImage"][..],
        ),
        (
            "reference_video",
            "ref_videos",
            "ref_video_",
            "ref_video",
            &["LoadVideo", "GetVideoComponents"][..],
        ),
    ] {
        let Some(binding) = config
            .bindings
            .get(name)
            .or_else(|| config.optional_bindings.get(name))
        else {
            continue;
        };
        for helper in helpers {
            if object_info.get(*helper).is_none() {
                missing_nodes.insert((*helper).to_owned());
            }
        }
        if binding.input != expected_input {
            errors.push(format!(
                "binding `{name}` must target H3 input `{expected_input}`, not `{}`",
                binding.input
            ));
            continue;
        }
        let Some(class_type) = workflow
            .get(&binding.node)
            .and_then(|node| node.get("class_type"))
            .and_then(Value::as_str)
        else {
            continue;
        };
        let Some(schema) = object_info
            .get(class_type)
            .and_then(|node_info| object_info_input(node_info, &binding.input))
        else {
            continue;
        };
        let input_type = schema.get(0).and_then(Value::as_str);
        let actual_prefix = schema.pointer("/1/template/prefix").and_then(Value::as_str);
        let slot_type = schema
            .pointer(&format!("/1/template/input/required/{slot}/0"))
            .and_then(Value::as_str);
        if input_type != Some("COMFY_AUTOGROW_V3")
            || actual_prefix != Some(prefix)
            || slot_type != Some("IMAGE")
        {
            errors.push(format!(
                "binding `{name}` does not match the installed H3 `{expected_input}` IMAGE autogrow schema"
            ));
        }
    }
}

pub(super) fn fill_operation_capabilities(
    report: &mut CapabilityReport,
    config: &ComfyAdapterConfig,
) {
    let has_binding = |name: &str| {
        config.bindings.contains_key(name) || config.optional_bindings.contains_key(name)
    };
    for (name, required) in [
        ("t2v", Vec::<&str>::new()),
        ("i2v", vec!["first_frame"]),
        ("flf2v", vec!["first_frame", "last_frame"]),
        ("r2v", Vec::new()),
    ] {
        let missing = required
            .into_iter()
            .filter(|binding| !has_binding(binding))
            .collect::<Vec<_>>();
        let r2v_reference_binding =
            name == "r2v" && !has_binding("reference_images") && !has_binding("reference_video");
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
        } else if !missing.is_empty() || r2v_reference_binding {
            (
                CapabilityStatus::Unsupported,
                if r2v_reference_binding {
                    "missing bindings: reference_images or reference_video".to_owned()
                } else {
                    format!("missing bindings: {}", missing.join(", "))
                },
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

pub(super) fn fill_unavailable_operations(report: &mut CapabilityReport, reason: &str) {
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

pub(super) fn history_state(entry: &Value) -> BackendState {
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

pub(super) fn queue_state(queue: &Value, prompt_id: &str) -> BackendState {
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

pub(super) fn parse_outputs(
    entry: &Value,
    output_node: &str,
) -> Result<Vec<OutputArtifact>, AdapterError> {
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

pub(super) fn validate_artifact(artifact: &OutputArtifact) -> Result<(), AdapterError> {
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

pub(super) fn validate_file_component(value: &str) -> Result<(), AdapterError> {
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

pub(super) fn validate_relative_path(path: &Path) -> Result<(), AdapterError> {
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

pub(super) fn validate_request_id(value: &str) -> Result<(), AdapterError> {
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

pub(super) fn is_loopback(url: &Url) -> bool {
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

pub(super) fn execution_error_message(data: &Value) -> String {
    data.get("exception_message")
        .and_then(Value::as_str)
        .or_else(|| data.get("exception_type").and_then(Value::as_str))
        .unwrap_or("ComfyUI execution failed")
        .to_owned()
}
