use serde::Serialize;

use super::*;

#[derive(Debug, Args)]
pub(super) struct ProjectArgs {
    #[command(subcommand)]
    pub(super) command: ProjectSubcommand,
}

#[derive(Debug, Subcommand)]
pub(super) enum ProjectSubcommand {
    /// List projects managed by the running worker.
    List {
        #[command(flatten)]
        connection: ConnectionArgs,
        #[arg(long)]
        json: bool,
    },
    /// Create a project through the running worker.
    New {
        #[arg(long, value_name = "PATH")]
        brief_file: PathBuf,
        #[arg(long, value_name = "PROJECT_ID")]
        id: Option<String>,
        #[arg(long, value_name = "TITLE")]
        title: Option<String>,
        #[command(flatten)]
        connection: ConnectionArgs,
        #[arg(long)]
        json: bool,
    },
    /// Read the latest project snapshot through the worker.
    Status {
        #[arg(long, value_name = "PROJECT_ID")]
        project: Option<String>,
        #[command(flatten)]
        connection: ConnectionArgs,
        #[arg(long)]
        json: bool,
    },
    /// Pause new work without interrupting the running job.
    Pause {
        #[arg(long, value_name = "PROJECT_ID")]
        project: String,
        #[command(flatten)]
        connection: ConnectionArgs,
        #[arg(long)]
        json: bool,
    },
    /// Resume scheduling new work for one project.
    Resume {
        #[arg(long, value_name = "PROJECT_ID")]
        project: String,
        #[command(flatten)]
        connection: ConnectionArgs,
        #[arg(long)]
        json: bool,
    },
    /// Validate a project and its internal hashes without changing it.
    Verify {
        #[arg(long, value_name = "PROJECT_ID")]
        project: String,
        #[arg(long, value_name = "PATH")]
        data_dir: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// Export a verified project to a checksummed TAR archive.
    Export {
        #[arg(long, value_name = "PROJECT_ID")]
        project: String,
        #[arg(long, value_name = "PATH")]
        output: PathBuf,
        #[arg(long, value_name = "PATH")]
        data_dir: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// Validate an archive manifest, payload hashes, and project contracts.
    VerifyArchive {
        #[arg(long, value_name = "PATH")]
        archive: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Verify and import an archive without overwriting an existing project.
    Import {
        #[arg(long, value_name = "PATH")]
        archive: PathBuf,
        #[arg(long, value_name = "PATH")]
        data_dir: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// Show a schema migration plan; modify files only with --apply.
    Migrate {
        #[arg(long, value_name = "PROJECT_ID")]
        project: String,
        #[arg(long, value_name = "PATH")]
        data_dir: Option<PathBuf>,
        #[arg(long)]
        apply: bool,
        #[arg(long)]
        json: bool,
    },
}

pub(super) fn execute_project(args: ProjectArgs) -> Result<ExitCode, CliError> {
    match args.command {
        ProjectSubcommand::List { connection, json } => {
            let client = WorkerClient::new(resolved_paths(&connection).socket, None);
            print_worker_reply(client.send(IpcCommand::ListProjects, None)?, json)
        }
        ProjectSubcommand::New {
            brief_file,
            id,
            title,
            connection,
            json,
        } => create_project(&brief_file, id, title, &connection, json),
        ProjectSubcommand::Status {
            project,
            connection,
            json,
        } => {
            let client = WorkerClient::new(resolved_paths(&connection).socket, project);
            print_worker_reply(client.send(IpcCommand::Snapshot, None)?, json)
        }
        ProjectSubcommand::Pause {
            project,
            connection,
            json,
        } => execute_project_pause(&project, &connection, json, true),
        ProjectSubcommand::Resume {
            project,
            connection,
            json,
        } => execute_project_pause(&project, &connection, json, false),
        ProjectSubcommand::Verify {
            project,
            data_dir,
            json,
        } => {
            let report = crate::portability::verify_project(
                &AppPaths::resolve(data_dir, None).projects_dir,
                &project,
            )?;
            print_report(
                &report,
                json,
                format!(
                    "VERIFIED {}: schema={}, revision={}, files={}, bytes={}",
                    report.project_id,
                    report.schema_version,
                    report.revision,
                    report.files,
                    report.bytes
                ),
            )
        }
        ProjectSubcommand::Export {
            project,
            output,
            data_dir,
            json,
        } => {
            let report = crate::portability::export_project(
                &AppPaths::resolve(data_dir, None).projects_dir,
                &project,
                &output,
            )?;
            print_report(
                &report,
                json,
                format!(
                    "EXPORTED {}: files={}, bytes={}, archive={}",
                    report.project_id,
                    report.files,
                    report.bytes,
                    output.display()
                ),
            )
        }
        ProjectSubcommand::VerifyArchive { archive, json } => {
            let report = crate::portability::verify_archive(&archive)?;
            print_report(
                &report,
                json,
                format!(
                    "ARCHIVE VERIFIED {}: schema={}, files={}, bytes={}",
                    report.project_id, report.schema_version, report.files, report.bytes
                ),
            )
        }
        ProjectSubcommand::Import {
            archive,
            data_dir,
            json,
        } => {
            let report = crate::portability::import_project(
                &AppPaths::resolve(data_dir, None).projects_dir,
                &archive,
            )?;
            print_report(
                &report,
                json,
                format!(
                    "IMPORTED {}: revision={}, files={}, bytes={}",
                    report.project_id, report.revision, report.files, report.bytes
                ),
            )
        }
        ProjectSubcommand::Migrate {
            project,
            data_dir,
            apply,
            json,
        } => execute_migration(&project, data_dir, apply, json),
    }
}

fn create_project(
    brief_file: &Path,
    id: Option<String>,
    title: Option<String>,
    connection: &ConnectionArgs,
    json: bool,
) -> Result<ExitCode, CliError> {
    let brief = read_text(brief_file)?;
    let project_id = match id {
        Some(id) => id,
        None => infer_project_id(brief_file)?,
    };
    let title = title.unwrap_or_else(|| infer_title(&brief, &project_id));
    let client = WorkerClient::new(resolved_paths(connection).socket, None);
    print_worker_reply(
        client.send(
            IpcCommand::CreateProject {
                project_id,
                title,
                brief,
            },
            None,
        )?,
        json,
    )
}

fn execute_project_pause(
    project: &str,
    connection: &ConnectionArgs,
    json: bool,
    paused: bool,
) -> Result<ExitCode, CliError> {
    let client = WorkerClient::new(resolved_paths(connection).socket, Some(project.to_owned()));
    let revision = current_revision(&client)?;
    let command = if paused {
        IpcCommand::PauseProject
    } else {
        IpcCommand::ResumeProject
    };
    print_worker_reply(client.send(command, Some(revision))?, json)
}

fn execute_migration(
    project: &str,
    data_dir: Option<PathBuf>,
    apply: bool,
    json: bool,
) -> Result<ExitCode, CliError> {
    let projects = AppPaths::resolve(data_dir, None).projects_dir;
    let preview = crate::portability::plan_migration(&projects, project)?;
    let report = if apply && preview.applicable {
        crate::portability::apply_migration(&projects, project)?
    } else {
        preview
    };
    let status = if !report.applicable {
        "UNSUPPORTED"
    } else if !report.required {
        "CURRENT"
    } else if apply {
        "APPLIED"
    } else {
        "DRY RUN"
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "MIGRATION {status} {}: {}/{} -> {}",
            report.project_id,
            report.project_schema_version,
            report.state_schema_version,
            report.target_schema_version
        );
        for change in &report.changes {
            println!("- {change}");
        }
        if let Some(path) = &report.backup_path {
            println!("backup: {}", path.display());
        }
    }
    Ok(if report.applicable {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(EXIT_INVALID)
    })
}

fn print_worker_reply(reply: WorkerReply, json: bool) -> Result<ExitCode, CliError> {
    print_reply(&reply, json)?;
    Ok(reply_exit_code(&reply))
}

fn print_report<T: Serialize>(
    report: &T,
    json: bool,
    message: String,
) -> Result<ExitCode, CliError> {
    if json {
        println!("{}", serde_json::to_string_pretty(report)?);
    } else {
        println!("{message}");
    }
    Ok(ExitCode::SUCCESS)
}
