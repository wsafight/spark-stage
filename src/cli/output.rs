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
    if let Some(payload) = &reply.payload {
        match payload {
            WorkerPayload::ProjectList { projects } => {
                for project in projects {
                    if let Some(error) = &project.error {
                        println!("{}: ERROR {error}", project.id);
                    } else {
                        println!(
                            "{}: stage={}, outcome={}, revision={}, paused={}",
                            project.id,
                            project.stage.as_deref().unwrap_or("unknown"),
                            project.outcome.as_deref().unwrap_or("unknown"),
                            project.revision.unwrap_or(0),
                            project.paused
                        );
                    }
                }
            }
            WorkerPayload::StorageReport(report) => {
                println!(
                    "{}: total={} bytes, trash={} bytes, reclaimable={} bytes in {} files",
                    report.project_id,
                    report.total_bytes,
                    report.trash_bytes,
                    report.reclaimable_bytes,
                    report.reclaimable_files
                );
            }
            WorkerPayload::CleanupPlan(plan) => {
                println!(
                    "{}: status={:?}, files={}, reclaimable={} bytes",
                    plan.plan_id,
                    plan.status,
                    plan.items.len(),
                    plan.reclaimable_bytes
                );
                for item in &plan.items {
                    println!(
                        "- {} {}: {} ({} bytes)",
                        item.kind,
                        item.subject_id,
                        item.path.display(),
                        item.bytes
                    );
                }
            }
            WorkerPayload::DecisionHistory { decisions } => {
                for decision in decisions {
                    println!(
                        "{} {} {} command={} at={}",
                        decision.event_id,
                        decision.kind,
                        decision.subject_id,
                        decision.command_id,
                        decision.occurred_at
                    );
                }
            }
        }
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
