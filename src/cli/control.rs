use std::process::ExitCode;

use clap::{Args, Subcommand};

use super::*;

#[derive(Debug, Args)]
pub(super) struct QueueArgs {
    #[command(subcommand)]
    command: QueueCommand,
}

#[derive(Debug, Subcommand)]
enum QueueCommand {
    /// Show queue pause state and all pending/running jobs.
    List(ProjectControlArgs),
    /// Prevent pending jobs from starting after the current safe boundary.
    Pause(ProjectControlArgs),
    /// Allow pending jobs to start.
    Resume(ProjectControlArgs),
    /// Cancel a pending job or an explicitly interruptible running job.
    Cancel {
        #[arg(long, value_name = "PROJECT_ID")]
        project: String,
        #[arg(long, value_name = "JOB_ID")]
        job: String,
        #[command(flatten)]
        connection: ConnectionArgs,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Args)]
pub(super) struct ApprovalArgs {
    #[command(subcommand)]
    command: ApprovalCommand,
}

#[derive(Debug, Subcommand)]
enum ApprovalCommand {
    /// Approve a script or build review gate by stable approval ID.
    Approve {
        #[arg(long, value_name = "PROJECT_ID")]
        project: String,
        #[arg(long, value_name = "APPROVAL_ID")]
        approval: String,
        #[command(flatten)]
        connection: ConnectionArgs,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Args)]
pub(super) struct DiagnosticsArgs {
    #[command(subcommand)]
    command: DiagnosticsCommand,
}

#[derive(Debug, Subcommand)]
enum DiagnosticsCommand {
    /// Refresh one named diagnostic probe.
    Retry {
        #[arg(long, value_name = "PROJECT_ID")]
        project: String,
        #[arg(long, value_name = "PROBE_ID")]
        probe: String,
        #[command(flatten)]
        connection: ConnectionArgs,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Args)]
pub(super) struct LogsArgs {
    #[command(subcommand)]
    command: LogsCommand,
}

#[derive(Debug, Subcommand)]
enum LogsCommand {
    /// Resolve the project log directory.
    Open(ProjectControlArgs),
}

#[derive(Debug, Args)]
struct ProjectControlArgs {
    #[arg(long, value_name = "PROJECT_ID")]
    project: String,
    #[command(flatten)]
    connection: ConnectionArgs,
    #[arg(long)]
    json: bool,
}

pub(super) fn execute_queue(args: QueueArgs) -> Result<ExitCode, CliError> {
    let (project, connection, command, json) = match args.command {
        QueueCommand::List(args) => control_command(args, IpcCommand::Snapshot),
        QueueCommand::Pause(args) => control_command(args, IpcCommand::PauseQueue),
        QueueCommand::Resume(args) => control_command(args, IpcCommand::ResumeQueue),
        QueueCommand::Cancel {
            project,
            job,
            connection,
            json,
        } => (
            project,
            connection,
            IpcCommand::CancelJob { job_id: job },
            json,
        ),
    };
    execute_control_command(project, connection, command, json)
}

pub(super) fn execute_approval(args: ApprovalArgs) -> Result<ExitCode, CliError> {
    let ApprovalCommand::Approve {
        project,
        approval,
        connection,
        json,
    } = args.command;
    execute_control_command(
        project,
        connection,
        IpcCommand::Approve {
            approval_id: approval,
        },
        json,
    )
}

pub(super) fn execute_diagnostics(args: DiagnosticsArgs) -> Result<ExitCode, CliError> {
    let DiagnosticsCommand::Retry {
        project,
        probe,
        connection,
        json,
    } = args.command;
    execute_control_command(
        project,
        connection,
        IpcCommand::RetryProbe { probe_id: probe },
        json,
    )
}

pub(super) fn execute_logs(args: LogsArgs) -> Result<ExitCode, CliError> {
    let LogsCommand::Open(args) = args.command;
    let (project, connection, command, json) = control_command(args, IpcCommand::OpenLogs);
    execute_control_command(project, connection, command, json)
}

fn control_command(
    args: ProjectControlArgs,
    command: IpcCommand,
) -> (String, ConnectionArgs, IpcCommand, bool) {
    (args.project, args.connection, command, args.json)
}

fn execute_control_command(
    project: String,
    connection: ConnectionArgs,
    command: IpcCommand,
    json: bool,
) -> Result<ExitCode, CliError> {
    let client = WorkerClient::new(resolved_paths(&connection).socket, Some(project));
    let expected_revision = command
        .is_mutating()
        .then(|| current_revision(&client))
        .transpose()?;
    let reply = client.send(command, expected_revision)?;
    print_reply(&reply, json)?;
    Ok(reply_exit_code(&reply))
}

#[cfg(test)]
mod tests {
    use std::os::unix::net::UnixListener;
    use std::thread;

    use super::*;
    use crate::ipc::{ClientRequest, IPC_PROTOCOL_VERSION, read_frame, write_frame};

    fn reply(request: &ClientRequest, revision: u64) -> WorkerReply {
        WorkerReply {
            protocol_version: IPC_PROTOCOL_VERSION.to_owned(),
            command_id: request.command_id.clone(),
            ok: true,
            revision: Some(revision),
            snapshot: None,
            payload: None,
            artifact_path: None,
            message: None,
            error: None,
        }
    }

    #[test]
    fn mutating_control_command_fetches_and_uses_current_revision() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("worker.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let worker = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let snapshot: ClientRequest = read_frame(&mut stream).unwrap();
            assert_eq!(snapshot.command, IpcCommand::Snapshot);
            assert_eq!(snapshot.expected_revision, None);
            write_frame(&mut stream, &reply(&snapshot, 9)).unwrap();

            let (mut stream, _) = listener.accept().unwrap();
            let command: ClientRequest = read_frame(&mut stream).unwrap();
            write_frame(&mut stream, &reply(&command, 9)).unwrap();
            command
        });

        let result = execute_control_command(
            "rain-apartment".to_owned(),
            ConnectionArgs {
                data_dir: None,
                socket: Some(socket),
            },
            IpcCommand::PauseQueue,
            false,
        )
        .unwrap();

        assert_eq!(result, ExitCode::SUCCESS);
        let command = worker.join().unwrap();
        assert_eq!(command.project_id.as_deref(), Some("rain-apartment"));
        assert_eq!(command.expected_revision, Some(9));
        assert_eq!(command.command, IpcCommand::PauseQueue);
    }
}
