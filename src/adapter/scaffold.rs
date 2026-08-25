use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use super::{AdapterError, ComfyAdapter, ComfyAdapterConfig, WorkflowBinding};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdapterScaffoldRequest {
    pub adapter: String,
    pub workflow: PathBuf,
    pub endpoint: String,
    pub allow_remote: bool,
    pub output_node: String,
    pub model_fingerprint: String,
    pub prompt: WorkflowBinding,
    pub seed: WorkflowBinding,
    pub output_prefix: WorkflowBinding,
    pub optional_bindings: BTreeMap<String, WorkflowBinding>,
}

pub fn scaffold_comfy_adapter(
    request: AdapterScaffoldRequest,
) -> Result<ComfyAdapterConfig, AdapterError> {
    if request.model_fingerprint.trim().is_empty() {
        return Err(AdapterError::Config(
            "model_fingerprint must not be empty".to_owned(),
        ));
    }
    for required in ["prompt", "seed", "output_prefix"] {
        if request.optional_bindings.contains_key(required) {
            return Err(AdapterError::Config(format!(
                "optional binding `{required}` duplicates a required binding"
            )));
        }
    }
    let workflow = canonical_file(&request.workflow)?;
    let bindings = BTreeMap::from([
        ("output_prefix".to_owned(), request.output_prefix),
        ("prompt".to_owned(), request.prompt),
        ("seed".to_owned(), request.seed),
    ]);
    reject_duplicate_targets(&bindings, &request.optional_bindings)?;
    let config = ComfyAdapterConfig {
        schema_version: "1.0".to_owned(),
        adapter: request.adapter,
        enabled: false,
        endpoint: request.endpoint,
        allow_remote: request.allow_remote,
        allow_global_interrupt: false,
        workflow,
        output_node: request.output_node,
        model_fingerprint: Some(request.model_fingerprint),
        bindings,
        optional_bindings: request.optional_bindings,
        profiles: BTreeMap::from([
            ("audition".to_owned(), BTreeMap::new()),
            ("baseline".to_owned(), BTreeMap::new()),
            ("final".to_owned(), BTreeMap::new()),
        ]),
        verified_operations: Vec::new(),
    };
    let adapter = ComfyAdapter::new(config.clone())?;
    adapter.validate_local_workflow()?;
    Ok(config)
}

pub fn parse_workflow_binding(value: &str) -> Result<WorkflowBinding, AdapterError> {
    let Some((node, input)) = value.split_once('.') else {
        return Err(AdapterError::Binding(format!(
            "`{value}` must use NODE.INPUT syntax"
        )));
    };
    if node.trim().is_empty() || input.trim().is_empty() || input.contains('.') {
        return Err(AdapterError::Binding(format!(
            "`{value}` must contain exactly one non-empty NODE.INPUT pair"
        )));
    }
    Ok(WorkflowBinding {
        node: node.to_owned(),
        input: input.to_owned(),
    })
}

pub fn parse_named_workflow_binding(
    value: &str,
) -> Result<(String, WorkflowBinding), AdapterError> {
    let Some((name, target)) = value.split_once('=') else {
        return Err(AdapterError::Binding(format!(
            "`{value}` must use NAME=NODE.INPUT syntax"
        )));
    };
    if name.is_empty()
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    {
        return Err(AdapterError::Binding(format!(
            "binding name `{name}` must use lowercase ASCII letters, digits, or underscores"
        )));
    }
    Ok((name.to_owned(), parse_workflow_binding(target)?))
}

fn canonical_file(path: &Path) -> Result<PathBuf, AdapterError> {
    let canonical = path.canonicalize().map_err(|source| AdapterError::Io {
        path: path.to_owned(),
        source,
    })?;
    if !canonical.is_file() {
        return Err(AdapterError::Config(format!(
            "workflow `{}` is not a regular file",
            path.display()
        )));
    }
    Ok(canonical)
}

fn reject_duplicate_targets(
    required: &BTreeMap<String, WorkflowBinding>,
    optional: &BTreeMap<String, WorkflowBinding>,
) -> Result<(), AdapterError> {
    let mut targets = HashSet::new();
    for (name, binding) in required.iter().chain(optional.iter()) {
        if !targets.insert((binding.node.as_str(), binding.input.as_str())) {
            return Err(AdapterError::Binding(format!(
                "binding `{name}` reuses target `{}.{}`",
                binding.node, binding.input
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::json;

    use super::*;

    fn workflow(directory: &Path) -> PathBuf {
        let path = directory.join("workflow.json");
        fs::write(
            &path,
            serde_json::to_vec(&json!({
                "45": {"class_type": "Text", "inputs": {"text": ""}},
                "78": {"class_type": "Seed", "inputs": {"noise_seed": 0}},
                "90": {"class_type": "Size", "inputs": {"width": 960}},
                "120": {"class_type": "Output", "inputs": {"filename_prefix": "out"}}
            }))
            .unwrap(),
        )
        .unwrap();
        path
    }

    fn request(workflow: PathBuf) -> AdapterScaffoldRequest {
        AdapterScaffoldRequest {
            adapter: "minimax-h3-comfy".to_owned(),
            workflow,
            endpoint: "http://127.0.0.1:8188".to_owned(),
            allow_remote: false,
            output_node: "120".to_owned(),
            model_fingerprint: "model-hash".to_owned(),
            prompt: parse_workflow_binding("45.text").unwrap(),
            seed: parse_workflow_binding("78.noise_seed").unwrap(),
            output_prefix: parse_workflow_binding("120.filename_prefix").unwrap(),
            optional_bindings: BTreeMap::from([(
                "width".to_owned(),
                parse_workflow_binding("90.width").unwrap(),
            )]),
        }
    }

    #[test]
    fn scaffold_is_disabled_and_declares_no_verified_operations() {
        let directory = tempfile::tempdir().unwrap();
        let config = scaffold_comfy_adapter(request(workflow(directory.path()))).unwrap();

        assert!(!config.enabled);
        assert!(config.verified_operations.is_empty());
        assert_eq!(config.bindings["prompt"].node, "45");
        assert!(config.workflow.is_absolute());
        assert_eq!(
            config
                .profiles
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["audition", "baseline", "final"]
        );
    }

    #[test]
    fn scaffold_rejects_missing_and_reused_explicit_targets() {
        let directory = tempfile::tempdir().unwrap();
        let path = workflow(directory.path());
        let mut missing = request(path.clone());
        missing.prompt = parse_workflow_binding("45.missing").unwrap();
        assert!(matches!(
            scaffold_comfy_adapter(missing),
            Err(AdapterError::Binding(_))
        ));

        let mut duplicate = request(path);
        duplicate.seed = duplicate.prompt.clone();
        assert!(matches!(
            scaffold_comfy_adapter(duplicate),
            Err(AdapterError::Binding(_))
        ));
    }

    #[test]
    fn binding_parser_requires_unambiguous_explicit_syntax() {
        assert_eq!(
            parse_named_workflow_binding("first_frame=12.image")
                .unwrap()
                .0,
            "first_frame"
        );
        for invalid in ["12", ".text", "12.", "12.input.extra", "Bad=12.image"] {
            let result = if invalid.contains('=') {
                parse_named_workflow_binding(invalid).map(|_| ())
            } else {
                parse_workflow_binding(invalid).map(|_| ())
            };
            assert!(result.is_err(), "{invalid} should be rejected");
        }
    }
}
