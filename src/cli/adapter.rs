use std::collections::BTreeMap;
use std::process::ExitCode;

use clap::{Args, Subcommand};

use super::*;
use crate::adapter::{
    AdapterScaffoldRequest, parse_named_workflow_binding, parse_workflow_binding,
    scaffold_comfy_adapter,
};

#[derive(Debug, Args)]
pub(super) struct AdapterArgs {
    #[command(subcommand)]
    command: AdapterCommand,
}

#[derive(Debug, Subcommand)]
enum AdapterCommand {
    /// Build a disabled YAML config from a ComfyUI API workflow and explicit bindings.
    Scaffold {
        #[arg(long, value_name = "PATH")]
        workflow: PathBuf,
        #[arg(long, value_name = "PATH")]
        output: PathBuf,
        #[arg(long, default_value = "minimax-h3-comfy")]
        name: String,
        #[arg(long, default_value = "http://127.0.0.1:8188")]
        endpoint: String,
        #[arg(long)]
        allow_remote: bool,
        #[arg(long, value_name = "NODE_ID")]
        output_node: String,
        #[arg(long, value_name = "FINGERPRINT")]
        model_fingerprint: String,
        #[arg(long, value_name = "NODE.INPUT")]
        prompt: String,
        #[arg(long, value_name = "NODE.INPUT")]
        seed: String,
        #[arg(long, value_name = "NODE.INPUT")]
        output_prefix: String,
        /// Add an explicit optional binding such as first_frame=90.image.
        #[arg(long = "binding", value_name = "NAME=NODE.INPUT")]
        optional_bindings: Vec<String>,
        /// Replace an existing output config.
        #[arg(long)]
        force: bool,
    },
}

pub(super) fn execute_adapter(args: AdapterArgs) -> Result<ExitCode, CliError> {
    let AdapterCommand::Scaffold {
        workflow,
        output,
        name,
        endpoint,
        allow_remote,
        output_node,
        model_fingerprint,
        prompt,
        seed,
        output_prefix,
        optional_bindings,
        force,
    } = args.command;
    if output.exists() && !force {
        return Err(CliError::InvalidInput(format!(
            "adapter config `{}` already exists; pass --force to replace it",
            output.display()
        )));
    }
    let mut parsed_optional = BTreeMap::new();
    for value in optional_bindings {
        let (binding_name, binding) = parse_named_workflow_binding(&value)?;
        if parsed_optional
            .insert(binding_name.clone(), binding)
            .is_some()
        {
            return Err(CliError::InvalidInput(format!(
                "binding `{binding_name}` was provided more than once"
            )));
        }
    }
    let config = scaffold_comfy_adapter(AdapterScaffoldRequest {
        adapter: name,
        workflow,
        endpoint,
        allow_remote,
        output_node,
        model_fingerprint,
        prompt: parse_workflow_binding(&prompt)?,
        seed: parse_workflow_binding(&seed)?,
        output_prefix: parse_workflow_binding(&output_prefix)?,
        optional_bindings: parsed_optional,
    })?;
    let encoded = serde_yaml_ng::to_string(&config)
        .map_err(|error| CliError::InvalidInput(format!("cannot encode adapter YAML: {error}")))?;
    write_atomic(&output, encoded.as_bytes())?;
    println!("{}", output.display());
    Ok(ExitCode::SUCCESS)
}
