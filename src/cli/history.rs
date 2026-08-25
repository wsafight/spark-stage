use clap::{Args, Subcommand};

use super::*;

#[derive(Debug, Args)]
pub(super) struct HistoryArgs {
    #[command(subcommand)]
    command: HistoryCommand,
}

#[derive(Debug, Subcommand)]
enum HistoryCommand {
    /// Read the append-only decision journal in newest-first order.
    Decisions {
        #[arg(long, value_name = "PROJECT_ID")]
        project: String,
        #[arg(
            long,
            value_name = "COUNT",
            default_value_t = 50,
            value_parser = clap::value_parser!(u32).range(1..=1_000)
        )]
        limit: u32,
        #[command(flatten)]
        connection: ConnectionArgs,
        #[arg(long)]
        json: bool,
    },
}

pub(super) fn execute_history(args: HistoryArgs) -> Result<ExitCode, CliError> {
    let HistoryCommand::Decisions {
        project,
        limit,
        connection,
        json,
    } = args.command;
    let client = WorkerClient::new(resolved_paths(&connection).socket, Some(project));
    let reply = client.send(IpcCommand::DecisionHistory { limit }, None)?;
    print_reply(&reply, json)?;
    Ok(reply_exit_code(&reply))
}
