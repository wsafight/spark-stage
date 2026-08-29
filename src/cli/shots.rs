use super::*;

pub(super) fn execute_shots(args: ShotsArgs) -> Result<ExitCode, CliError> {
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
        ShotsCommand::SmokeTest {
            project,
            shot,
            seed,
            accept_unverified: _,
            connection,
            json,
        } => (
            project,
            connection,
            IpcCommand::SmokeTestShot {
                shot_id: shot,
                seed,
            },
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
        ShotsCommand::Review {
            project,
            file,
            approve,
            connection,
            json,
        } => {
            let source = read_text(&file)?;
            let selections = serde_json::from_str(&source).map_err(|error| {
                CliError::InvalidInput(format!(
                    "cannot decode review file `{}`: {error}",
                    file.display()
                ))
            })?;
            (
                project,
                connection,
                IpcCommand::ReviewBatch {
                    selections,
                    approve,
                },
                json,
            )
        }
    };
    let client = WorkerClient::new(resolved_paths(&connection).socket, Some(project));
    let expected_revision = command
        .is_mutating()
        .then(|| current_revision(&client))
        .transpose()?;
    let reply = client.send(command, expected_revision)?;
    print_reply(&reply, json)?;
    Ok(reply_exit_code(&reply))
}
