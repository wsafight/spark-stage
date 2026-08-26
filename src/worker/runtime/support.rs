use super::*;

pub(super) fn success(
    request: &ClientRequest,
    revision: Option<u64>,
    snapshot: Option<AppSnapshot>,
    message: &str,
) -> WorkerReply {
    WorkerReply {
        protocol_version: IPC_PROTOCOL_VERSION.to_owned(),
        command_id: request.command_id.clone(),
        ok: true,
        revision,
        snapshot,
        payload: None,
        artifact_path: None,
        message: Some(message.to_owned()),
        error: None,
    }
}

pub(super) fn success_payload(
    request: &ClientRequest,
    revision: Option<u64>,
    payload: WorkerPayload,
    message: &str,
) -> WorkerReply {
    WorkerReply {
        protocol_version: IPC_PROTOCOL_VERSION.to_owned(),
        command_id: request.command_id.clone(),
        ok: true,
        revision,
        snapshot: None,
        payload: Some(payload),
        artifact_path: None,
        message: Some(message.to_owned()),
        error: None,
    }
}

pub(super) fn failure(
    request: &ClientRequest,
    code: &str,
    message: String,
    retryable: bool,
    revision: Option<u64>,
) -> WorkerReply {
    WorkerReply {
        protocol_version: IPC_PROTOCOL_VERSION.to_owned(),
        command_id: request.command_id.clone(),
        ok: false,
        revision,
        snapshot: None,
        payload: None,
        artifact_path: None,
        message: None,
        error: Some(WorkerError {
            code: code.to_owned(),
            message,
            retryable,
            current_revision: revision,
        }),
    }
}

pub(super) fn missing_revision(request: &ClientRequest) -> WorkerReply {
    failure(
        request,
        "EXPECTED_REVISION_REQUIRED",
        "mutating project commands require expected_revision".to_owned(),
        false,
        None,
    )
}

pub(super) fn store_failure(request: &ClientRequest, error: StoreError) -> WorkerReply {
    match error {
        StoreError::RevisionConflict { actual, .. } => failure(
            request,
            "REVISION_CONFLICT",
            "project state changed; refresh before retrying".to_owned(),
            true,
            Some(actual),
        ),
        StoreError::ProjectNotFound(_) => {
            failure(request, "PROJECT_NOT_FOUND", error.to_string(), false, None)
        }
        StoreError::ProjectExists(_) => {
            failure(request, "PROJECT_EXISTS", error.to_string(), false, None)
        }
        StoreError::InvalidProjectId(_) | StoreError::ProjectIdMismatch { .. } => {
            failure(request, "PROJECT_INVALID", error.to_string(), false, None)
        }
        StoreError::LockBusy { .. } => {
            failure(request, "RESOURCE_BUSY", error.to_string(), true, None)
        }
        StoreError::NoPendingScriptApproval
        | StoreError::ApprovalNotFound(_)
        | StoreError::ContractNotFound(_) => failure(
            request,
            "APPROVAL_NOT_FOUND",
            error.to_string(),
            false,
            None,
        ),
        StoreError::NoActiveContract => failure(
            request,
            "ACTIVE_CONTRACT_REQUIRED",
            error.to_string(),
            false,
            None,
        ),
        StoreError::ShotNotFound(_) => {
            failure(request, "SHOT_NOT_FOUND", error.to_string(), false, None)
        }
        StoreError::ShotBusy { .. } => {
            failure(request, "SHOT_BUSY", error.to_string(), false, None)
        }
        StoreError::BuildBusy { .. } => {
            failure(request, "BUILD_BUSY", error.to_string(), false, None)
        }
        StoreError::InvalidJob(_) => {
            failure(request, "JOB_INVALID", error.to_string(), false, None)
        }
        StoreError::JobNotCancellable { .. } => failure(
            request,
            "JOB_NOT_CANCELLABLE",
            error.to_string(),
            false,
            None,
        ),
        StoreError::TakeNotFound(_) => {
            failure(request, "TAKE_NOT_FOUND", error.to_string(), false, None)
        }
        StoreError::TakeShotMismatch { .. } => failure(
            request,
            "TAKE_SHOT_MISMATCH",
            error.to_string(),
            false,
            None,
        ),
        StoreError::TakeStale(_) => failure(request, "TAKE_STALE", error.to_string(), false, None),
        StoreError::TakeRejected(_) => {
            failure(request, "TAKE_REJECTED", error.to_string(), false, None)
        }
        StoreError::TakeNotSelected(_) => {
            failure(request, "TAKE_NOT_SELECTED", error.to_string(), false, None)
        }
        StoreError::ShotAlreadyApproved(_) => failure(
            request,
            "SHOT_ALREADY_APPROVED",
            error.to_string(),
            false,
            None,
        ),
        StoreError::TakeUnavailable(_) => {
            failure(request, "TAKE_UNAVAILABLE", error.to_string(), false, None)
        }
        StoreError::InvalidReviewBatch(_) => failure(
            request,
            "REVIEW_BATCH_INVALID",
            error.to_string(),
            false,
            None,
        ),
        StoreError::ReviewWarningsNotAccepted(_) => failure(
            request,
            "REVIEW_WARNINGS_NOT_ACCEPTED",
            error.to_string(),
            false,
            None,
        ),
        StoreError::InvalidHistoryLimit(_) => failure(
            request,
            "HISTORY_LIMIT_INVALID",
            error.to_string(),
            false,
            None,
        ),
        StoreError::CleanupPlanNotFound(_) => failure(
            request,
            "CLEANUP_PLAN_NOT_FOUND",
            error.to_string(),
            false,
            None,
        ),
        StoreError::InvalidCleanupPlan(_) => failure(
            request,
            "INVALID_CLEANUP_PLAN",
            error.to_string(),
            false,
            None,
        ),
        StoreError::CleanupPlanStale(_) => failure(
            request,
            "CLEANUP_PLAN_STALE",
            error.to_string(),
            false,
            None,
        ),
        StoreError::CleanupPathConflict(_) => failure(
            request,
            "CLEANUP_PATH_CONFLICT",
            error.to_string(),
            false,
            None,
        ),
        StoreError::ReferenceSubjectNotFound { .. } => failure(
            request,
            "REFERENCE_SUBJECT_NOT_FOUND",
            error.to_string(),
            false,
            None,
        ),
        StoreError::ReferenceNotFound(_) => failure(
            request,
            "REFERENCE_NOT_FOUND",
            error.to_string(),
            false,
            None,
        ),
        StoreError::InvalidReferenceSource(_) => failure(
            request,
            "REFERENCE_SOURCE_INVALID",
            error.to_string(),
            false,
            None,
        ),
        StoreError::ReferenceImpactConfirmationRequired => failure(
            request,
            "REFERENCE_IMPACT_CONFIRMATION_REQUIRED",
            error.to_string(),
            false,
            None,
        ),
        StoreError::ReferenceIntegrity(_) => failure(
            request,
            "REFERENCE_INTEGRITY_FAILED",
            error.to_string(),
            false,
            None,
        ),
        _ => failure(request, "STORE_ERROR", error.to_string(), false, None),
    }
}

pub(super) fn worker_failure(request: &ClientRequest, error: WorkerDomainError) -> WorkerReply {
    match error {
        WorkerDomainError::Store(error) => store_failure(request, error),
        WorkerDomainError::ProjectRequired(projects) => failure(
            request,
            "PROJECT_REQUIRED",
            format!("select a project; available projects: {projects:?}"),
            false,
            None,
        ),
        WorkerDomainError::Io { .. } => {
            failure(request, "STORE_ERROR", error.to_string(), false, None)
        }
    }
}

pub(super) fn timestamp() -> String {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}.{:03}Z", duration.as_secs(), duration.subsec_millis())
}

impl WorkerRuntime {
    pub(super) fn emit_milestone(
        &self,
        kind: MilestoneKind,
        project_id: impl Into<String>,
        subject_id: impl Into<String>,
        message: impl Into<String>,
    ) {
        if let Some(hooks) = &self.hooks {
            hooks.emit(MilestoneEvent::new(
                kind,
                project_id,
                subject_id,
                message,
                timestamp(),
            ));
        }
    }

    pub(super) fn emit_command_milestone(&self, request: &ClientRequest, reply: &WorkerReply) {
        let project_id = request.project_id.clone().unwrap_or_default();
        if reply
            .error
            .as_ref()
            .is_some_and(|error| error.code == "DISK_BUDGET_EXCEEDED")
        {
            self.emit_milestone(
                MilestoneKind::DiskBlocked,
                project_id,
                request.command_id.clone(),
                reply
                    .error
                    .as_ref()
                    .map_or_else(String::new, |error| error.message.clone()),
            );
            return;
        }
        if !reply.ok {
            return;
        }
        if matches!(request.command, WorkerCommand::ApplyScript { .. })
            || reply
                .message
                .as_ref()
                .is_some_and(|message| message.contains("budget approval"))
        {
            let approval_id = reply
                .snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.pending_approvals.last())
                .map_or_else(
                    || request.command_id.clone(),
                    |approval| approval.approval_id.clone(),
                );
            self.emit_milestone(
                MilestoneKind::ApprovalRequired,
                project_id,
                approval_id,
                reply.message.clone().unwrap_or_default(),
            );
        } else if matches!(request.command, WorkerCommand::Approve { .. })
            && reply.snapshot.as_ref().is_some_and(|snapshot| {
                matches!(
                    snapshot.project.outcome.as_str(),
                    "done" | "done_with_warnings"
                )
            })
        {
            self.emit_milestone(
                MilestoneKind::ProjectCompleted,
                project_id,
                request.command_id.clone(),
                "project completed",
            );
        }
    }
}

pub(super) fn recovery_project_id(request: &ClientRequest) -> Option<String> {
    match &request.command {
        WorkerCommand::CreateProject { project_id, .. } => Some(project_id.clone()),
        _ => request.project_id.clone(),
    }
}

pub(super) const fn operation_name(operation: Operation) -> &'static str {
    match operation {
        Operation::T2v => "t2v",
        Operation::I2v => "i2v",
        Operation::Flf2v => "flf2v",
        Operation::R2v => "r2v",
    }
}

pub(super) const fn operation_bindings(operation: Operation) -> &'static [&'static str] {
    match operation {
        Operation::T2v => &[],
        Operation::I2v => &["first_frame"],
        Operation::Flf2v => &["first_frame", "last_frame"],
        Operation::R2v => &["reference_video"],
    }
}

pub(super) const fn command_kind(command: &WorkerCommand) -> &'static str {
    match command {
        WorkerCommand::Health => "health",
        WorkerCommand::ListProjects => "project.list",
        WorkerCommand::Snapshot => "snapshot",
        WorkerCommand::Subscribe { .. } => "revision.subscribe",
        WorkerCommand::CreateProject { .. } => "project.create",
        WorkerCommand::PauseProject => "project.pause",
        WorkerCommand::ResumeProject => "project.resume",
        WorkerCommand::UpdateBudget { .. } => "budget.update",
        WorkerCommand::StorageStatus => "storage.status",
        WorkerCommand::CreateCleanupPlan => "storage.plan",
        WorkerCommand::ApplyCleanupPlan { .. } => "storage.apply",
        WorkerCommand::RestoreCleanupPlan { .. } => "storage.restore",
        WorkerCommand::ReviewBatch { .. } => "take.review_batch",
        WorkerCommand::DecisionHistory { .. } => "history.decisions",
        WorkerCommand::ListReferences => "reference.list",
        WorkerCommand::ReferenceImpact { .. } => "reference.impact",
        WorkerCommand::ImportReference { .. } => "reference.import",
        WorkerCommand::ReplaceReference { .. } => "reference.replace",
        WorkerCommand::VerifyReferences => "reference.verify",
        WorkerCommand::ApplyScript { .. } => "script.apply",
        WorkerCommand::ApproveScript => "script.approve",
        WorkerCommand::Approve { .. } => "approval.approve",
        WorkerCommand::RetryShot { .. } => "shot.retry",
        WorkerCommand::AuditionShot { .. } => "shot.audition",
        WorkerCommand::RenderShot { .. } => "shot.render",
        WorkerCommand::SelectTake { .. } => "take.select",
        WorkerCommand::ApproveTake { .. } => "take.approve",
        WorkerCommand::RejectTake { .. } => "take.reject",
        WorkerCommand::PreviewTake { .. } => "take.preview",
        WorkerCommand::PauseQueue => "queue.pause",
        WorkerCommand::ResumeQueue => "queue.resume",
        WorkerCommand::CancelJob { .. } => "job.cancel",
        WorkerCommand::Build { .. } => "build.create",
        WorkerCommand::OpenBuild { .. } => "build.open",
        WorkerCommand::RetryProbe { .. } => "probe.retry",
        WorkerCommand::OpenLogs => "logs.open",
    }
}

pub(super) const fn project_stage(value: ProjectStage) -> &'static str {
    match value {
        ProjectStage::Authoring => "authoring",
        ProjectStage::Shooting => "shooting",
        ProjectStage::Review => "review",
        ProjectStage::Build => "build",
        ProjectStage::Completed => "completed",
    }
}

pub(super) const fn project_outcome(value: ProjectOutcome) -> &'static str {
    match value {
        ProjectOutcome::InProgress => "in_progress",
        ProjectOutcome::NeedsReview => "needs_review",
        ProjectOutcome::Done => "done",
        ProjectOutcome::DoneWithWarnings => "done_with_warnings",
        ProjectOutcome::Failed => "failed",
        ProjectOutcome::Cancelled => "cancelled",
    }
}

pub(super) const fn work_mode(value: WorkMode) -> &'static str {
    match value {
        WorkMode::Fast => "fast",
        WorkMode::Director => "director",
    }
}

pub(super) const fn quality_target(value: QualityTarget) -> &'static str {
    match value {
        QualityTarget::DraftCut => "draft_cut",
        QualityTarget::Playable => "playable",
        QualityTarget::Approved => "approved",
    }
}

pub(super) const fn approval_kind(value: ApprovalKind) -> &'static str {
    match value {
        ApprovalKind::ScriptBundle => "script_bundle",
        ApprovalKind::CandidateSelection => "candidate_selection",
        ApprovalKind::BudgetOverrun => "budget_overrun",
        ApprovalKind::BuildReview => "build_review",
        ApprovalKind::FinalVisualReview => "final_visual_review",
    }
}

pub(super) const fn shot_stage(value: ShotStage) -> &'static str {
    match value {
        ShotStage::Pending => "pending",
        ShotStage::Queued => "queued",
        ShotStage::Generating => "generating",
        ShotStage::CandidatesReady => "candidates_ready",
        ShotStage::Selected => "selected",
        ShotStage::Approved => "approved",
        ShotStage::Failed => "failed",
    }
}

pub(super) const fn risk(value: Risk) -> &'static str {
    match value {
        Risk::Low => "low",
        Risk::Medium => "medium",
        Risk::High => "high",
    }
}
