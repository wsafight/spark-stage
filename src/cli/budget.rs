use super::*;
use crate::domain::BudgetContract;

#[derive(Debug, Args)]
pub(super) struct BudgetArgs {
    #[command(subcommand)]
    command: BudgetCommand,
}

#[derive(Debug, Subcommand)]
enum BudgetCommand {
    /// Show the current budget estimate and limits in the project snapshot.
    Status(BudgetTarget),
    /// Replace the project's budget contract from a JSON file.
    Apply {
        #[command(flatten)]
        target: BudgetTarget,
        #[arg(long, value_name = "PATH")]
        contract: PathBuf,
    },
    /// Print or write the conservative unmeasured default contract.
    Default {
        #[arg(long, value_name = "PATH")]
        output: Option<PathBuf>,
    },
}

#[derive(Debug, Args)]
struct BudgetTarget {
    #[arg(long, value_name = "PROJECT_ID")]
    project: String,
    #[command(flatten)]
    connection: ConnectionArgs,
    #[arg(long)]
    json: bool,
}

pub(super) fn execute_budget(args: BudgetArgs) -> Result<ExitCode, CliError> {
    match args.command {
        BudgetCommand::Status(target) => {
            let client = WorkerClient::new(
                resolved_paths(&target.connection).socket,
                Some(target.project),
            );
            let reply = client.send(IpcCommand::Snapshot, None)?;
            print_reply(&reply, target.json)?;
            Ok(reply_exit_code(&reply))
        }
        BudgetCommand::Apply { target, contract } => {
            let source = read_text(&contract)?;
            let contract: BudgetContract = serde_json::from_str(&source)?;
            contract
                .validate()
                .map_err(|error| CliError::InvalidInput(error.to_string()))?;
            let client = WorkerClient::new(
                resolved_paths(&target.connection).socket,
                Some(target.project),
            );
            let revision = current_revision(&client)?;
            let reply = client.send(IpcCommand::UpdateBudget { contract }, Some(revision))?;
            print_reply(&reply, target.json)?;
            Ok(reply_exit_code(&reply))
        }
        BudgetCommand::Default { output } => {
            let mut encoded = serde_json::to_string_pretty(&BudgetContract::default())?;
            encoded.push('\n');
            if let Some(path) = output {
                write_atomic(&path, encoded.as_bytes())?;
            } else {
                print!("{encoded}");
            }
            Ok(ExitCode::SUCCESS)
        }
    }
}
