use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use clap::{Args, Parser, Subcommand};
use serde::Serialize;
use thiserror::Error;

use crate::ipc::{WorkerClient, WorkerCommand as IpcCommand, WorkerPayload, WorkerReply};
use crate::paths::AppPaths;
use crate::validation::{ValidationIssue, json_schema, validate_json};

mod adapter;
mod benchmark;
mod budget;
mod control;
mod edit;
mod history;
mod output;
mod project;
mod shots;
mod storage;

use adapter::{AdapterArgs, execute_adapter};
use benchmark::{BenchmarkArgs, execute_benchmark};
use budget::{BudgetArgs, execute_budget};
use control::{
    ApprovalArgs, DiagnosticsArgs, LogsArgs, QueueArgs, execute_approval, execute_diagnostics,
    execute_logs, execute_queue,
};
use edit::execute_edit;
use history::{HistoryArgs, execute_history};
use output::{print_reply, reply_exit_code};
use project::{ProjectArgs, execute_project};
use shots::execute_shots;
use storage::{StorageArgs, execute_storage};

#[cfg(test)]
use edit::expand_shot_selection;
#[cfg(test)]
use project::ProjectSubcommand;

const EXIT_ERROR: u8 = 1;
const EXIT_INVALID: u8 = 2;

#[derive(Debug, Parser)]
#[command(
    name = "sparkstage",
    version,
    about = "SparkStage production control plane"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Check the local runtime and camera workflow without changing state.
    Preflight(PreflightArgs),
    /// Run or inspect the single-writer service.
    Worker(WorkerArgs),
    /// Create and inspect local production projects.
    Project(ProjectArgs),
    /// Work with externally authored script contracts.
    Script(ScriptArgs),
    /// Queue camera work for approved shots.
    Shots(ShotsArgs),
    /// Assemble reviewed takes into draft, trailer, or final outputs.
    Edit(EditArgs),
    /// Inspect or control the shared production queue.
    Queue(QueueArgs),
    /// Resolve a pending approval by its stable ID.
    Approval(ApprovalArgs),
    /// Refresh worker diagnostic probes.
    Diagnostics(DiagnosticsArgs),
    /// Resolve project logs for inspection.
    Logs(LogsArgs),
    /// Inspect project storage and apply recoverable cleanup plans.
    Storage(StorageArgs),
    /// Inspect append-only project decision history.
    History(HistoryArgs),
    /// Inspect or replace the persisted project budget contract.
    Budget(BudgetArgs),
    /// Prepare and inspect immutable MiniMax H3 benchmark records.
    Benchmark(BenchmarkArgs),
    /// Generate a disabled adapter config from explicit workflow bindings.
    Adapter(AdapterArgs),
    /// Open the Ratatui production console connected to the worker.
    Tui(TuiArgs),
}

#[derive(Debug, Args)]
struct PreflightArgs {
    /// Adapter config to inspect; may be repeated.
    #[arg(long = "adapter-config", value_name = "PATH")]
    adapter_configs: Vec<PathBuf>,
    /// Override the SparkStage application data directory used for the disk probe.
    #[arg(long, value_name = "PATH")]
    data_dir: Option<PathBuf>,
    /// Minimum free space required for production media.
    #[arg(long, value_name = "GIB", default_value_t = 50)]
    minimum_free_gib: u64,
    /// Emit a stable machine-readable report.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Clone, Args)]
struct ConnectionArgs {
    /// Override the SparkStage application data directory.
    #[arg(long, value_name = "PATH")]
    data_dir: Option<PathBuf>,
    /// Override the worker Unix socket path.
    #[arg(long, value_name = "PATH")]
    socket: Option<PathBuf>,
}

#[derive(Debug, Args)]
struct WorkerArgs {
    #[command(subcommand)]
    command: WorkerSubcommand,
}

#[derive(Debug, Subcommand)]
enum WorkerSubcommand {
    /// Run the foreground single-writer worker.
    Run {
        #[command(flatten)]
        connection: ConnectionArgs,
        /// Camera adapter config used for generation commands.
        #[arg(long, value_name = "PATH")]
        adapter_config: Option<PathBuf>,
    },
    /// Check whether the worker is accepting commands.
    Status {
        #[command(flatten)]
        connection: ConnectionArgs,
        /// Emit the stable worker reply envelope.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Args)]
struct TuiArgs {
    /// Override the worker Unix socket path.
    #[arg(long, value_name = "PATH")]
    socket: Option<PathBuf>,
    /// Select a project when the worker manages more than one.
    #[arg(long, value_name = "PROJECT_ID")]
    project: Option<String>,
    /// Snapshot polling interval in milliseconds.
    #[arg(
        long,
        value_name = "MILLISECONDS",
        default_value_t = 1000,
        value_parser = clap::value_parser!(u64).range(100..=60_000)
    )]
    refresh_ms: u64,
}

#[derive(Debug, Args)]
struct ScriptArgs {
    #[command(subcommand)]
    command: ScriptCommand,
}

#[derive(Debug, Args)]
struct ShotsArgs {
    #[command(subcommand)]
    command: ShotsCommand,
}

#[derive(Debug, Args)]
struct EditArgs {
    #[command(subcommand)]
    command: EditCommand,
}

#[derive(Debug, Subcommand)]
enum EditCommand {
    /// Build a dynamic draft or the final full-length cut.
    Build {
        #[arg(long, value_name = "PROJECT_ID")]
        project: String,
        #[arg(long, value_parser = ["draft", "final"], default_value = "final")]
        kind: String,
        /// Build a draft from a comma-separated shot list or range such as S04-S07,S10.
        #[arg(long, value_name = "SHOT_SELECTION")]
        shots: Option<String>,
        #[command(flatten)]
        connection: ConnectionArgs,
        #[arg(long)]
        json: bool,
    },
    /// Build a deterministic two-second-per-shot trailer montage.
    Trailer {
        #[arg(long, value_name = "PROJECT_ID")]
        project: String,
        #[command(flatten)]
        connection: ConnectionArgs,
        #[arg(long)]
        json: bool,
    },
    /// Resolve an immutable build output for preview or automation.
    Open {
        #[arg(long, value_name = "PROJECT_ID")]
        project: String,
        #[arg(long, value_name = "BUILD_ID")]
        build: String,
        #[command(flatten)]
        connection: ConnectionArgs,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Subcommand)]
enum ShotsCommand {
    /// Queue one low-cost candidate take.
    Audition {
        #[arg(long, value_name = "PROJECT_ID")]
        project: String,
        #[arg(long, value_name = "SHOT_ID")]
        shot: String,
        #[command(flatten)]
        connection: ConnectionArgs,
        #[arg(long)]
        json: bool,
    },
    /// Queue one final-profile take.
    Render {
        #[arg(long, value_name = "PROJECT_ID")]
        project: String,
        #[arg(long, value_name = "SHOT_ID")]
        shot: String,
        #[command(flatten)]
        connection: ConnectionArgs,
        #[arg(long)]
        json: bool,
    },
    /// Retry a pending or failed shot using its next appropriate profile.
    Retry {
        #[arg(long, value_name = "PROJECT_ID")]
        project: String,
        #[arg(long, value_name = "SHOT_ID")]
        shot: String,
        #[command(flatten)]
        connection: ConnectionArgs,
        #[arg(long)]
        json: bool,
    },
    /// Select one generated take for a shot.
    Select {
        #[arg(long, value_name = "PROJECT_ID")]
        project: String,
        #[arg(long, value_name = "SHOT_ID")]
        shot: String,
        #[arg(long, value_name = "TAKE_ID")]
        take: String,
        #[command(flatten)]
        connection: ConnectionArgs,
        #[arg(long)]
        json: bool,
    },
    /// Approve the selected take for a shot.
    Approve {
        #[arg(long, value_name = "PROJECT_ID")]
        project: String,
        #[arg(long, value_name = "SHOT_ID")]
        shot: String,
        #[arg(long, value_name = "TAKE_ID")]
        take: String,
        #[command(flatten)]
        connection: ConnectionArgs,
        #[arg(long)]
        json: bool,
    },
    /// Reject one generated take for a shot.
    Reject {
        #[arg(long, value_name = "PROJECT_ID")]
        project: String,
        #[arg(long, value_name = "SHOT_ID")]
        shot: String,
        #[arg(long, value_name = "TAKE_ID")]
        take: String,
        #[command(flatten)]
        connection: ConnectionArgs,
        #[arg(long)]
        json: bool,
    },
    /// Resolve one generated take for preview or automation.
    Preview {
        #[arg(long, value_name = "PROJECT_ID")]
        project: String,
        #[arg(long, value_name = "TAKE_ID")]
        take: String,
        #[command(flatten)]
        connection: ConnectionArgs,
        #[arg(long)]
        json: bool,
    },
    /// Select multiple takes atomically, optionally approving them in the same revision.
    Review {
        #[arg(long, value_name = "PROJECT_ID")]
        project: String,
        #[arg(long, value_name = "PATH")]
        file: PathBuf,
        #[arg(long)]
        approve: bool,
        #[command(flatten)]
        connection: ConnectionArgs,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Subcommand)]
enum ScriptCommand {
    /// Validate a ScriptBundle without changing project state.
    Validate {
        #[arg(value_name = "BUNDLE")]
        bundle: PathBuf,
        /// Emit a stable machine-readable report.
        #[arg(long)]
        json: bool,
    },
    /// Print or write the current ScriptBundle JSON Schema.
    Schema {
        /// Write the schema to a file instead of stdout.
        #[arg(long, value_name = "PATH")]
        output: Option<PathBuf>,
    },
    /// Import a valid ScriptBundle through the worker and request approval.
    Apply {
        #[arg(value_name = "BUNDLE")]
        bundle: PathBuf,
        #[arg(long, value_name = "PROJECT_ID")]
        project: String,
        #[command(flatten)]
        connection: ConnectionArgs,
        /// Emit the stable worker reply envelope.
        #[arg(long)]
        json: bool,
    },
    /// Approve the pending ScriptBundle and make it active.
    Approve {
        #[arg(long, value_name = "PROJECT_ID")]
        project: String,
        #[command(flatten)]
        connection: ConnectionArgs,
        /// Emit the stable worker reply envelope.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Serialize)]
struct MachineReport<'a> {
    valid: bool,
    code: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    summary: Option<ValidationSummary<'a>>,
    errors: &'a [ValidationIssue],
}

#[derive(Debug, Serialize)]
struct ValidationSummary<'a> {
    project_id: &'a str,
    schema_version: &'a str,
    shots: usize,
    duration_seconds: u32,
}

#[derive(Debug, Error)]
enum CliError {
    #[error("cannot read `{path}`: {source}")]
    Read { path: PathBuf, source: io::Error },
    #[error("cannot write `{path}`: {source}")]
    Write { path: PathBuf, source: io::Error },
    #[error("cannot serialize JSON output: {0}")]
    Serialize(#[from] serde_json::Error),
    #[error(transparent)]
    Tui(#[from] crate::tui::TuiError),
    #[error(transparent)]
    Worker(#[from] crate::worker::WorkerRunError),
    #[error(transparent)]
    Client(#[from] crate::ipc::ClientError),
    #[error(transparent)]
    Benchmark(#[from] crate::benchmark::BenchmarkError),
    #[error(transparent)]
    Adapter(#[from] crate::adapter::AdapterError),
    #[error(transparent)]
    Portability(#[from] crate::portability::PortabilityError),
    #[error("cannot initialize async runtime: {0}")]
    Runtime(#[from] io::Error),
    #[error("{0}")]
    InvalidInput(String),
}

pub fn run() -> ExitCode {
    match execute(Cli::parse()) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("ERROR: {error}");
            ExitCode::from(EXIT_ERROR)
        }
    }
}

fn execute(cli: Cli) -> Result<ExitCode, CliError> {
    match cli.command {
        Command::Preflight(args) => execute_preflight(args),
        Command::Worker(args) => execute_worker(args),
        Command::Project(args) => execute_project(args),
        Command::Script(args) => match args.command {
            ScriptCommand::Validate { bundle, json } => validate_file(&bundle, json),
            ScriptCommand::Schema { output } => write_schema(output.as_deref()),
            ScriptCommand::Apply {
                bundle,
                project,
                connection,
                json,
            } => apply_script(&bundle, &project, &connection, json),
            ScriptCommand::Approve {
                project,
                connection,
                json,
            } => approve_script(&project, &connection, json),
        },
        Command::Shots(args) => execute_shots(args),
        Command::Edit(args) => execute_edit(args),
        Command::Queue(args) => execute_queue(args),
        Command::Approval(args) => execute_approval(args),
        Command::Diagnostics(args) => execute_diagnostics(args),
        Command::Logs(args) => execute_logs(args),
        Command::Storage(args) => execute_storage(args),
        Command::History(args) => execute_history(args),
        Command::Budget(args) => execute_budget(args),
        Command::Benchmark(args) => execute_benchmark(args),
        Command::Adapter(args) => execute_adapter(args),
        Command::Tui(args) => {
            crate::tui::run(crate::tui::TuiOptions {
                socket: args.socket.unwrap_or_else(crate::tui::default_socket_path),
                project_id: args.project,
                refresh_interval: Duration::from_millis(args.refresh_ms),
            })?;
            Ok(ExitCode::SUCCESS)
        }
    }
}

fn execute_worker(args: WorkerArgs) -> Result<ExitCode, CliError> {
    match args.command {
        WorkerSubcommand::Run {
            connection,
            adapter_config,
        } => {
            let paths = resolved_paths(&connection);
            println!("Starting SparkStage worker at {}", paths.socket.display());
            crate::worker::run(crate::worker::WorkerOptions {
                paths,
                adapter_config: adapter_config
                    .or_else(|| std::env::var_os("SPARKSTAGE_ADAPTER_CONFIG").map(PathBuf::from))
                    .or_else(|| {
                        let default = PathBuf::from("adapters/minimax-h3-comfy.yaml");
                        default.exists().then_some(default)
                    }),
            })?;
            Ok(ExitCode::SUCCESS)
        }
        WorkerSubcommand::Status { connection, json } => {
            let client = WorkerClient::new(resolved_paths(&connection).socket, None);
            let reply = client.send(IpcCommand::Health, None)?;
            print_reply(&reply, json)?;
            Ok(reply_exit_code(&reply))
        }
    }
}

fn apply_script(
    path: &Path,
    project: &str,
    connection: &ConnectionArgs,
    json: bool,
) -> Result<ExitCode, CliError> {
    let source = read_text(path)?;
    let validation = validate_json(&source);
    if !validation.is_valid() {
        return validate_file(path, json);
    }
    let client = WorkerClient::new(resolved_paths(connection).socket, Some(project.to_owned()));
    let revision = current_revision(&client)?;
    let reply = client.send(
        IpcCommand::ApplyScript {
            bundle_json: source,
        },
        Some(revision),
    )?;
    print_reply(&reply, json)?;
    Ok(reply_exit_code(&reply))
}

fn approve_script(
    project: &str,
    connection: &ConnectionArgs,
    json: bool,
) -> Result<ExitCode, CliError> {
    let client = WorkerClient::new(resolved_paths(connection).socket, Some(project.to_owned()));
    let revision = current_revision(&client)?;
    let reply = client.send(IpcCommand::ApproveScript, Some(revision))?;
    print_reply(&reply, json)?;
    Ok(reply_exit_code(&reply))
}

fn current_revision(client: &WorkerClient) -> Result<u64, CliError> {
    client
        .send(IpcCommand::Snapshot, None)?
        .revision
        .ok_or_else(|| CliError::InvalidInput("worker snapshot has no revision".to_owned()))
}

fn resolved_paths(connection: &ConnectionArgs) -> AppPaths {
    let socket = connection
        .socket
        .clone()
        .or_else(|| std::env::var_os("SPARKSTAGE_SOCKET").map(PathBuf::from));
    AppPaths::resolve(connection.data_dir.clone(), socket)
}

fn read_text(path: &Path) -> Result<String, CliError> {
    fs::read_to_string(path).map_err(|source| CliError::Read {
        path: path.to_owned(),
        source,
    })
}

fn infer_project_id(path: &Path) -> Result<String, CliError> {
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    let mut slug = String::new();
    let mut separator = false;
    for character in stem.chars().flat_map(char::to_lowercase) {
        if character.is_ascii_alphanumeric() {
            slug.push(character);
            separator = false;
        } else if !slug.is_empty() && !separator {
            slug.push('-');
            separator = true;
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    if slug.is_empty() {
        Err(CliError::InvalidInput(
            "cannot infer an ASCII project id; pass --id".to_owned(),
        ))
    } else {
        Ok(slug)
    }
}

fn infer_title(brief: &str, project_id: &str) -> String {
    brief
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| line.trim_start_matches('#').trim().to_owned())
        .filter(|line| !line.is_empty())
        .unwrap_or_else(|| project_id.replace('-', " "))
}

fn execute_preflight(args: PreflightArgs) -> Result<ExitCode, CliError> {
    let paths = AppPaths::resolve(args.data_dir, None);
    let adapter_configs = discover_adapter_configs(args.adapter_configs);
    let minimum_free_bytes = args.minimum_free_gib.saturating_mul(1024_u64.pow(3));
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let report = runtime.block_on(crate::preflight::run(
        &paths.data_home,
        &adapter_configs,
        minimum_free_bytes,
    ));

    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "SparkStage preflight: {}",
            if report.ready { "READY" } else { "NOT READY" }
        );
        for check in &report.checks {
            println!("- {:?} {}: {}", check.status, check.code, check.detail);
        }
    }

    Ok(if report.ready {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(EXIT_INVALID)
    })
}

fn discover_adapter_configs(explicit: Vec<PathBuf>) -> Vec<PathBuf> {
    if !explicit.is_empty() {
        return explicit;
    }
    if let Some(path) = std::env::var_os("SPARKSTAGE_ADAPTER_CONFIG") {
        return vec![PathBuf::from(path)];
    }
    let default = PathBuf::from("adapters/minimax-h3-comfy.yaml");
    default.exists().then_some(default).into_iter().collect()
}

fn validate_file(path: &Path, machine_readable: bool) -> Result<ExitCode, CliError> {
    let source = fs::read_to_string(path).map_err(|source| CliError::Read {
        path: path.to_owned(),
        source,
    })?;
    let result = validate_json(&source);

    if machine_readable {
        let summary = result.bundle.as_ref().map(|bundle| ValidationSummary {
            project_id: &bundle.project.id,
            schema_version: &bundle.schema_version,
            shots: bundle.shots.len(),
            duration_seconds: bundle.shots.iter().map(|shot| shot.duration).sum(),
        });
        let report = MachineReport {
            valid: result.is_valid(),
            code: if result.is_valid() {
                "SCRIPT_BUNDLE_VALID"
            } else {
                "SCRIPT_BUNDLE_INVALID"
            },
            summary,
            errors: &result.issues,
        };
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else if let Some(bundle) = &result.bundle {
        println!(
            "VALID {}: {} shots, {} seconds",
            bundle.project.id,
            bundle.shots.len(),
            bundle.shots.iter().map(|shot| shot.duration).sum::<u32>()
        );
    } else {
        eprintln!(
            "SCRIPT_BUNDLE_INVALID {} ({} errors)",
            path.display(),
            result.issues.len()
        );
        for issue in &result.issues {
            let path = if issue.path.is_empty() {
                "/"
            } else {
                &issue.path
            };
            eprintln!("- {} {path}: {}", issue.code, issue.message);
        }
    }

    Ok(if result.is_valid() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(EXIT_INVALID)
    })
}

fn write_schema(output: Option<&Path>) -> Result<ExitCode, CliError> {
    let mut encoded = serde_json::to_string_pretty(&json_schema())?;
    encoded.push('\n');

    if let Some(path) = output {
        write_atomic(path, encoded.as_bytes())?;
    } else {
        print!("{encoded}");
    }

    Ok(ExitCode::SUCCESS)
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), CliError> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent).map_err(|source| CliError::Write {
            path: parent.to_owned(),
            source,
        })?;
    }

    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("schema.json");
    let temporary = path.with_file_name(format!(".{file_name}.{}.tmp", std::process::id()));

    let write_result = (|| {
        let mut file = fs::File::create(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        Ok::<(), io::Error>(())
    })();

    if write_result.is_err() {
        let _ = fs::remove_file(&temporary);
    }

    write_result.map_err(|source| CliError::Write {
        path: path.to_owned(),
        source,
    })
}

#[cfg(test)]
mod tests;
