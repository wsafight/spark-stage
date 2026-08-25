use super::*;

#[derive(Debug, Args)]
pub(super) struct StorageArgs {
    #[command(subcommand)]
    command: StorageCommand,
}

#[derive(Debug, Subcommand)]
enum StorageCommand {
    /// Show total, trash, and safely reclaimable bytes.
    Status(StorageTarget),
    /// Create an immutable cleanup plan without moving files.
    Plan(StorageTarget),
    /// Move all files in a cleanup plan into the project trash directory.
    Apply {
        #[command(flatten)]
        target: StorageTarget,
        #[arg(long, value_name = "PLAN_ID")]
        plan: String,
    },
    /// Restore all files in an applied cleanup plan.
    Restore {
        #[command(flatten)]
        target: StorageTarget,
        #[arg(long, value_name = "PLAN_ID")]
        plan: String,
    },
}

#[derive(Debug, Args)]
struct StorageTarget {
    #[arg(long, value_name = "PROJECT_ID")]
    project: String,
    #[command(flatten)]
    connection: ConnectionArgs,
    #[arg(long)]
    json: bool,
}

pub(super) fn execute_storage(args: StorageArgs) -> Result<ExitCode, CliError> {
    let (target, command) = match args.command {
        StorageCommand::Status(target) => (target, IpcCommand::StorageStatus),
        StorageCommand::Plan(target) => (target, IpcCommand::CreateCleanupPlan),
        StorageCommand::Apply { target, plan } => {
            (target, IpcCommand::ApplyCleanupPlan { plan_id: plan })
        }
        StorageCommand::Restore { target, plan } => {
            (target, IpcCommand::RestoreCleanupPlan { plan_id: plan })
        }
    };
    let client = WorkerClient::new(
        resolved_paths(&target.connection).socket,
        Some(target.project),
    );
    let revision = command
        .is_mutating()
        .then(|| current_revision(&client))
        .transpose()?;
    let reply = client.send(command, revision)?;
    print_reply(&reply, target.json)?;
    Ok(reply_exit_code(&reply))
}
