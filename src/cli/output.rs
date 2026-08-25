use super::*;

pub(super) fn print_reply(reply: &WorkerReply, machine_readable: bool) -> Result<(), CliError> {
    if machine_readable {
        println!("{}", serde_json::to_string_pretty(reply)?);
        return Ok(());
    }
    if let Some(message) = &reply.message {
        println!("{message}");
    }
    if let Some(path) = &reply.artifact_path {
        println!("{}", path.display());
    }
    if let Some(error) = &reply.error {
        eprintln!("{}: {}", error.code, error.message);
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

pub(super) fn reply_exit_code(reply: &WorkerReply) -> ExitCode {
    if reply.ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(EXIT_INVALID)
    }
}
