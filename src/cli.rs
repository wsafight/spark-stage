use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use clap::{Args, Parser, Subcommand};
use serde::Serialize;
use thiserror::Error;

use crate::ipc::{WorkerClient, WorkerCommand as IpcCommand, WorkerReply};
use crate::paths::AppPaths;
use crate::validation::{ValidationIssue, json_schema, validate_json};

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
struct ProjectArgs {
    #[command(subcommand)]
    command: ProjectSubcommand,
}

#[derive(Debug, Subcommand)]
enum ProjectSubcommand {
    /// Create a project through the running worker.
    New {
        #[arg(long, value_name = "PATH")]
        brief_file: PathBuf,
        /// Stable lowercase project slug; defaults to the brief file stem.
        #[arg(long, value_name = "PROJECT_ID")]
        id: Option<String>,
        /// Display title; defaults to the first non-empty brief line.
        #[arg(long, value_name = "TITLE")]
        title: Option<String>,
        #[command(flatten)]
        connection: ConnectionArgs,
        /// Emit the stable worker reply envelope.
        #[arg(long)]
        json: bool,
    },
    /// Read the latest project snapshot through the worker.
    Status {
        #[arg(long, value_name = "PROJECT_ID")]
        project: Option<String>,
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

fn execute_shots(args: ShotsArgs) -> Result<ExitCode, CliError> {
    let (project, connection, command, json) = match args.command {
        ShotsCommand::Audition {
            project,
            shot,
            connection,
            json,
        } => (
            project,
            connection,
            IpcCommand::AuditionShot { shot_id: shot },
            json,
        ),
        ShotsCommand::Render {
            project,
            shot,
            connection,
            json,
        } => (
            project,
            connection,
            IpcCommand::RenderShot { shot_id: shot },
            json,
        ),
        ShotsCommand::Retry {
            project,
            shot,
            connection,
            json,
        } => (
            project,
            connection,
            IpcCommand::RetryShot { shot_id: shot },
            json,
        ),
        ShotsCommand::Select {
            project,
            shot,
            take,
            connection,
            json,
        } => (
            project,
            connection,
            IpcCommand::SelectTake {
                shot_id: shot,
                take_id: take,
            },
            json,
        ),
        ShotsCommand::Approve {
            project,
            shot,
            take,
            connection,
            json,
        } => (
            project,
            connection,
            IpcCommand::ApproveTake {
                shot_id: shot,
                take_id: take,
            },
            json,
        ),
        ShotsCommand::Reject {
            project,
            shot,
            take,
            connection,
            json,
        } => (
            project,
            connection,
            IpcCommand::RejectTake {
                shot_id: shot,
                take_id: take,
            },
            json,
        ),
        ShotsCommand::Preview {
            project,
            take,
            connection,
            json,
        } => (
            project,
            connection,
            IpcCommand::PreviewTake { take_id: take },
            json,
        ),
    };
    let client = WorkerClient::new(resolved_paths(&connection).socket, Some(project));
    let expected_revision = if command.is_mutating() {
        Some(current_revision(&client)?)
    } else {
        None
    };
    let reply = client.send(command, expected_revision)?;
    print_reply(&reply, json)?;
    Ok(if reply.ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(EXIT_INVALID)
    })
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
            Ok(ExitCode::SUCCESS)
        }
    }
}

fn execute_project(args: ProjectArgs) -> Result<ExitCode, CliError> {
    match args.command {
        ProjectSubcommand::New {
            brief_file,
            id,
            title,
            connection,
            json,
        } => {
            let brief = read_text(&brief_file)?;
            let project_id = match id {
                Some(id) => id,
                None => infer_project_id(&brief_file)?,
            };
            let title = title.unwrap_or_else(|| infer_title(&brief, &project_id));
            let client = WorkerClient::new(resolved_paths(&connection).socket, None);
            let reply = client.send(
                IpcCommand::CreateProject {
                    project_id,
                    title,
                    brief,
                },
                None,
            )?;
            print_reply(&reply, json)?;
            Ok(ExitCode::SUCCESS)
        }
        ProjectSubcommand::Status {
            project,
            connection,
            json,
        } => {
            let client = WorkerClient::new(resolved_paths(&connection).socket, project);
            let reply = client.send(IpcCommand::Snapshot, None)?;
            print_reply(&reply, json)?;
            Ok(ExitCode::SUCCESS)
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
    Ok(ExitCode::SUCCESS)
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
    Ok(ExitCode::SUCCESS)
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

fn print_reply(reply: &WorkerReply, machine_readable: bool) -> Result<(), CliError> {
    if machine_readable {
        println!("{}", serde_json::to_string_pretty(reply)?);
        return Ok(());
    }
    if let Some(message) = &reply.message {
        println!("{message}");
    }
    if let Some(snapshot) = &reply.snapshot {
        println!(
            "{}: stage={}, outcome={}, revision={}, shots={}, approvals={}",
            snapshot.project.id,
            snapshot.project.stage,
            snapshot.project.outcome,
            snapshot.revision,
            snapshot.shots.len(),
            snapshot.pending_approvals.len()
        );
    }
    Ok(())
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
mod tests {
    use super::*;

    #[test]
    fn schema_write_is_atomic_and_parseable() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("nested/script-bundle.schema.json");

        assert_eq!(write_schema(Some(&path)).unwrap(), ExitCode::SUCCESS);
        let value: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(value["title"], "ScriptBundle");
    }

    #[test]
    fn tui_arguments_parse_without_starting_terminal() {
        let cli = Cli::try_parse_from([
            "sparkstage",
            "tui",
            "--socket",
            "/tmp/sparkstage-test.sock",
            "--project",
            "rain-apartment",
            "--refresh-ms",
            "250",
        ])
        .unwrap();

        let Command::Tui(args) = cli.command else {
            panic!("expected tui command");
        };
        assert_eq!(
            args.socket,
            Some(PathBuf::from("/tmp/sparkstage-test.sock"))
        );
        assert_eq!(args.project.as_deref(), Some("rain-apartment"));
        assert_eq!(args.refresh_ms, 250);
    }

    #[test]
    fn shot_decision_commands_parse_stable_ids() {
        let cli = Cli::try_parse_from([
            "sparkstage",
            "shots",
            "select",
            "--project",
            "rain-apartment",
            "--shot",
            "S01",
            "--take",
            "TAKE-01ARZ3NDEKTSV4RRFFQ69G5FAV",
            "--json",
        ])
        .unwrap();

        let Command::Shots(ShotsArgs {
            command:
                ShotsCommand::Select {
                    project,
                    shot,
                    take,
                    json,
                    ..
                },
        }) = cli.command
        else {
            panic!("expected shots select command");
        };
        assert_eq!(project, "rain-apartment");
        assert_eq!(shot, "S01");
        assert_eq!(take, "TAKE-01ARZ3NDEKTSV4RRFFQ69G5FAV");
        assert!(json);
    }
}
