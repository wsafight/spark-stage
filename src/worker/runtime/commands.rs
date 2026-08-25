use super::*;

mod approvals;
mod cancellation;
mod diagnostics;

impl WorkerRuntime {
    pub(super) fn execute(&mut self, request: &ClientRequest) -> WorkerReply {
        match &request.command {
            WorkerCommand::Health => success(request, None, None, "worker is ready"),
            WorkerCommand::Snapshot | WorkerCommand::Subscribe { .. } => {
                match self.snapshot_for(request.project_id.as_deref()) {
                    Ok(snapshot) => success(
                        request,
                        Some(snapshot.revision),
                        Some(snapshot),
                        "snapshot loaded",
                    ),
                    Err(error) => worker_failure(request, error),
                }
            }
            WorkerCommand::CreateProject {
                project_id,
                title,
                brief,
            } => self.create_project(request, project_id, title, brief),
            WorkerCommand::ApplyScript { bundle_json } => self.apply_script(request, bundle_json),
            WorkerCommand::ApproveScript => self.approve_script(request, None),
            WorkerCommand::Approve { approval_id } => self.approve(request, approval_id),
            WorkerCommand::AuditionShot { shot_id } => self.enqueue_shot(request, shot_id, true),
            WorkerCommand::RenderShot { shot_id } => self.enqueue_shot(request, shot_id, false),
            WorkerCommand::RetryShot { shot_id } => self.retry_shot(request, shot_id),
            WorkerCommand::SelectTake { shot_id, take_id } => {
                self.mutate_take(request, shot_id, take_id, TakeMutation::Select)
            }
            WorkerCommand::ApproveTake { shot_id, take_id } => {
                self.mutate_take(request, shot_id, take_id, TakeMutation::Approve)
            }
            WorkerCommand::RejectTake { shot_id, take_id } => {
                self.mutate_take(request, shot_id, take_id, TakeMutation::Reject)
            }
            WorkerCommand::PreviewTake { take_id } => self.preview_take(request, take_id),
            WorkerCommand::PauseQueue => self.set_queue_paused(request, true),
            WorkerCommand::ResumeQueue => self.set_queue_paused(request, false),
            WorkerCommand::CancelJob { job_id } => self.cancel_job(request, job_id),
            WorkerCommand::Build { kind, shot_ids } => self.build(request, kind, shot_ids),
            WorkerCommand::OpenBuild { build_id } => self.open_build(request, build_id),
            WorkerCommand::RetryProbe { probe_id } => self.retry_probe(request, probe_id),
            WorkerCommand::OpenLogs => self.open_logs(request),
        }
    }

    fn build(&mut self, request: &ClientRequest, kind: &str, shot_ids: &[String]) -> WorkerReply {
        let Some(expected_revision) = request.expected_revision else {
            return missing_revision(request);
        };
        let store = match self.project_store(request.project_id.as_deref()) {
            Ok(store) => store,
            Err(error) => return worker_failure(request, error),
        };
        let state = match store.read_state() {
            Ok(state) => state,
            Err(error) => return store_failure(request, error),
        };
        if state.revision != expected_revision {
            return store_failure(
                request,
                StoreError::RevisionConflict {
                    expected: expected_revision,
                    actual: state.revision,
                },
            );
        }
        if let Some(approval) = state
            .pending_approvals
            .iter()
            .find(|approval| approval.blocking)
        {
            return failure(
                request,
                "APPROVAL_REQUIRED",
                format!(
                    "resolve approval `{}` before building",
                    approval.approval_id
                ),
                false,
                Some(state.revision),
            );
        }
        let kind = match crate::build::BuildKind::parse(kind) {
            Ok(kind) => kind,
            Err(error) => {
                return failure(
                    request,
                    "BUILD_KIND_INVALID",
                    error.to_string(),
                    false,
                    Some(state.revision),
                );
            }
        };
        if !shot_ids.is_empty() && kind != crate::build::BuildKind::Draft {
            return failure(
                request,
                "BUILD_SCOPE_INVALID",
                "a shot selection is only valid for a draft build; final and trailer builds must cover the full contract"
                    .to_owned(),
                false,
                Some(state.revision),
            );
        }
        let bundle = match store.read_active_bundle() {
            Ok(Some(bundle)) => bundle,
            Ok(None) => {
                return failure(
                    request,
                    "ACTIVE_CONTRACT_REQUIRED",
                    "approve a ScriptBundle before building".to_owned(),
                    false,
                    Some(state.revision),
                );
            }
            Err(error) => return store_failure(request, error),
        };
        let build_id = format!("BLD-{}", Ulid::new());
        let recipe = match crate::build::plan_selected(&build_id, kind, &state, &bundle, shot_ids) {
            Ok(recipe) => recipe,
            Err(error) => {
                return failure(
                    request,
                    "BUILD_INPUT_INVALID",
                    error.to_string(),
                    false,
                    Some(state.revision),
                );
            }
        };
        for input in &recipe.inputs {
            let media = store.root().join(&input.media_path);
            if !media.is_file() {
                return failure(
                    request,
                    "BUILD_INPUT_NOT_FOUND",
                    format!(
                        "take `{}` media is missing at {}",
                        input.take_id,
                        media.display()
                    ),
                    false,
                    Some(state.revision),
                );
            }
        }
        match crate::build::missing_runtime_capabilities() {
            Ok(missing) if !missing.is_empty() => {
                return failure(
                    request,
                    "BUILD_CAPABILITY_MISSING",
                    format!(
                        "ffmpeg build capabilities are missing: {}",
                        missing.join(", ")
                    ),
                    false,
                    Some(state.revision),
                );
            }
            Ok(_) => {}
            Err(error) => {
                return failure(
                    request,
                    "BUILD_CAPABILITY_PROBE_FAILED",
                    error.to_string(),
                    true,
                    Some(state.revision),
                );
            }
        }
        let recipe_path = PathBuf::from("builds").join(&build_id).join("recipe.json");
        if let Err(error) = write_json_atomic(&store.root().join(&recipe_path), &recipe) {
            return store_failure(request, error);
        }
        let build = crate::domain::BuildRecord {
            build_id: build_id.clone(),
            kind: kind.as_str().to_owned(),
            status: "queued".to_owned(),
            recipe: recipe_path.to_string_lossy().into_owned(),
            command_id: request.command_id.clone(),
            output_path: None,
            warnings: Vec::new(),
            stale: false,
        };
        let running =
            match store.start_build(build, expected_revision, &request.command_id, &timestamp()) {
                Ok(state) => state,
                Err(error) => return store_failure(request, error),
            };
        if let Err(message) = self.build_executor.send(BuildRequest {
            project_id: state.project_id.clone(),
            project_root: store.root().to_owned(),
            command_id: request.command_id.clone(),
            recipe,
        }) {
            let failed = match store.finish_build(
                &build_id,
                None,
                Some(message.clone()),
                false,
                running.revision,
                &request.command_id,
                &timestamp(),
            ) {
                Ok(state) => state,
                Err(error) => return store_failure(request, error),
            };
            return failure(
                request,
                "BUILD_EXECUTOR_UNAVAILABLE",
                message,
                true,
                Some(failed.revision),
            );
        }
        match self.snapshot(&store, running) {
            Ok(snapshot) => success(
                request,
                Some(snapshot.revision),
                Some(snapshot),
                "build queued",
            ),
            Err(error) => worker_failure(request, error),
        }
    }

    fn open_build(&self, request: &ClientRequest, build_id: &str) -> WorkerReply {
        let store = match self.project_store(request.project_id.as_deref()) {
            Ok(store) => store,
            Err(error) => return worker_failure(request, error),
        };
        let state = match store.read_state() {
            Ok(state) => state,
            Err(error) => return store_failure(request, error),
        };
        let Some(build) = state.builds.get(build_id) else {
            return failure(
                request,
                "BUILD_NOT_FOUND",
                format!("build `{build_id}` does not exist"),
                false,
                Some(state.revision),
            );
        };
        let Some(relative) = build.output_path.as_ref() else {
            return failure(
                request,
                "BUILD_OUTPUT_MISSING",
                format!("build `{build_id}` has no output"),
                false,
                Some(state.revision),
            );
        };
        if relative.is_absolute()
            || relative
                .components()
                .any(|component| !matches!(component, Component::Normal(_) | Component::CurDir))
        {
            return failure(
                request,
                "ARTIFACT_PATH_INVALID",
                format!("build `{build_id}` has an unsafe output path"),
                false,
                Some(state.revision),
            );
        }
        let output = store.root().join(relative);
        if !output.is_file() {
            return failure(
                request,
                "ARTIFACT_NOT_FOUND",
                format!("build output is missing at {}", output.display()),
                false,
                Some(state.revision),
            );
        }
        WorkerReply {
            protocol_version: IPC_PROTOCOL_VERSION.to_owned(),
            command_id: request.command_id.clone(),
            ok: true,
            revision: Some(state.revision),
            snapshot: None,
            artifact_path: Some(output),
            message: Some("build output ready".to_owned()),
            error: None,
        }
    }

    fn create_project(
        &mut self,
        request: &ClientRequest,
        project_id: &str,
        title: &str,
        brief: &str,
    ) -> WorkerReply {
        if request.expected_revision.is_some() {
            return failure(
                request,
                "INVALID_ARGUMENT",
                "project creation must not include expected_revision".to_owned(),
                false,
                None,
            );
        }
        if request
            .project_id
            .as_deref()
            .is_some_and(|requested| requested != project_id)
        {
            return failure(
                request,
                "PROJECT_ID_MISMATCH",
                "request project_id does not match create payload".to_owned(),
                false,
                None,
            );
        }
        if title.trim().is_empty() || brief.trim().is_empty() {
            return failure(
                request,
                "INVALID_ARGUMENT",
                "project title and brief must not be empty".to_owned(),
                false,
                None,
            );
        }
        match ProjectStore::create(
            &self.paths.projects_dir,
            project_id,
            title.trim(),
            brief,
            &request.command_id,
            &timestamp(),
        ) {
            Ok(store) => match store.read_state() {
                Ok(state) => match self.snapshot(&store, state) {
                    Ok(snapshot) => success(
                        request,
                        Some(snapshot.revision),
                        Some(snapshot),
                        "project created",
                    ),
                    Err(error) => worker_failure(request, error),
                },
                Err(error) => store_failure(request, error),
            },
            Err(error) => store_failure(request, error),
        }
    }

    fn apply_script(&mut self, request: &ClientRequest, bundle_json: &str) -> WorkerReply {
        let Some(expected_revision) = request.expected_revision else {
            return missing_revision(request);
        };
        let validation = validate_json(bundle_json);
        let Some(bundle) = validation.bundle else {
            let message = validation
                .issues
                .iter()
                .map(|issue| {
                    format!(
                        "{} {}: {}",
                        issue.code,
                        if issue.path.is_empty() {
                            "/"
                        } else {
                            &issue.path
                        },
                        issue.message
                    )
                })
                .collect::<Vec<_>>()
                .join("; ");
            return failure(
                request,
                "SCRIPT_BUNDLE_INVALID",
                message,
                false,
                self.project_revision(request.project_id.as_deref()),
            );
        };
        let store = match self.project_store(request.project_id.as_deref()) {
            Ok(store) => store,
            Err(error) => return worker_failure(request, error),
        };
        match store.apply_bundle(
            &bundle,
            expected_revision,
            &request.command_id,
            &timestamp(),
        ) {
            Ok((state, approval)) => match self.snapshot(&store, state) {
                Ok(snapshot) => success(
                    request,
                    Some(snapshot.revision),
                    Some(snapshot),
                    &format!("script bundle awaits approval {}", approval.approval_id),
                ),
                Err(error) => worker_failure(request, error),
            },
            Err(error) => store_failure(request, error),
        }
    }

    fn approve_script(
        &mut self,
        request: &ClientRequest,
        approval_id: Option<&str>,
    ) -> WorkerReply {
        let Some(expected_revision) = request.expected_revision else {
            return missing_revision(request);
        };
        let store = match self.project_store(request.project_id.as_deref()) {
            Ok(store) => store,
            Err(error) => return worker_failure(request, error),
        };
        match store.approve_script(
            approval_id,
            expected_revision,
            &request.command_id,
            &timestamp(),
        ) {
            Ok(state) => match self.snapshot(&store, state) {
                Ok(snapshot) => success(
                    request,
                    Some(snapshot.revision),
                    Some(snapshot),
                    "script bundle approved",
                ),
                Err(error) => worker_failure(request, error),
            },
            Err(error) => store_failure(request, error),
        }
    }

    fn set_queue_paused(&mut self, request: &ClientRequest, paused: bool) -> WorkerReply {
        let Some(expected_revision) = request.expected_revision else {
            return missing_revision(request);
        };
        let store = match self.project_store(request.project_id.as_deref()) {
            Ok(store) => store,
            Err(error) => return worker_failure(request, error),
        };
        let state = match store.read_state() {
            Ok(state) => state,
            Err(error) => return store_failure(request, error),
        };
        if state.revision != expected_revision {
            return store_failure(
                request,
                StoreError::RevisionConflict {
                    expected: expected_revision,
                    actual: state.revision,
                },
            );
        }
        let Some(next_revision) = self.queue.revision.checked_add(1) else {
            return failure(
                request,
                "QUEUE_REVISION_OVERFLOW",
                "queue revision cannot be incremented".to_owned(),
                false,
                Some(state.revision),
            );
        };
        let mut next_queue = self.queue.clone();
        next_queue.paused = paused;
        next_queue.revision = next_revision;
        next_queue.last_command_id = Some(request.command_id.clone());
        if let Err(error) = write_json_atomic(&self.paths.queue_file(), &next_queue) {
            return store_failure(request, error);
        }
        self.queue = next_queue;
        match self.snapshot(&store, state) {
            Ok(snapshot) => success(
                request,
                Some(snapshot.revision),
                Some(snapshot),
                if paused {
                    "queue paused"
                } else {
                    "queue resumed"
                },
            ),
            Err(error) => worker_failure(request, error),
        }
    }

    fn enqueue_shot(
        &mut self,
        request: &ClientRequest,
        shot_id: &str,
        audition: bool,
    ) -> WorkerReply {
        let Some(expected_revision) = request.expected_revision else {
            return missing_revision(request);
        };
        let store = match self.project_store(request.project_id.as_deref()) {
            Ok(store) => store,
            Err(error) => return worker_failure(request, error),
        };
        let state = match store.read_state() {
            Ok(state) => state,
            Err(error) => return store_failure(request, error),
        };
        if state.revision != expected_revision {
            return store_failure(
                request,
                StoreError::RevisionConflict {
                    expected: expected_revision,
                    actual: state.revision,
                },
            );
        }
        let blocking_approval = state.pending_approvals.iter().find(|approval| {
            if !approval.blocking {
                return false;
            }
            match approval.kind {
                ApprovalKind::ScriptBundle
                | ApprovalKind::BudgetOverrun
                | ApprovalKind::BuildReview => true,
                ApprovalKind::CandidateSelection => {
                    !audition && approval.shot_id.as_deref() == Some(shot_id)
                }
                ApprovalKind::FinalVisualReview => false,
            }
        });
        if let Some(approval) = blocking_approval {
            return failure(
                request,
                "APPROVAL_REQUIRED",
                format!(
                    "approval `{}` must be resolved before this camera job is queued",
                    approval.approval_id
                ),
                false,
                Some(state.revision),
            );
        }
        let bundle = match store.read_active_bundle() {
            Ok(Some(bundle)) => bundle,
            Ok(None) => {
                return failure(
                    request,
                    "ACTIVE_CONTRACT_REQUIRED",
                    "approve a ScriptBundle before queuing camera work".to_owned(),
                    false,
                    Some(state.revision),
                );
            }
            Err(error) => return store_failure(request, error),
        };
        let Some(shot) = bundle.shots.iter().find(|shot| shot.id == shot_id) else {
            return failure(
                request,
                "SHOT_NOT_FOUND",
                format!("shot `{shot_id}` is not in the active contract"),
                false,
                Some(state.revision),
            );
        };
        let profile = if audition {
            &shot.generation_plan.audition_profile
        } else {
            &shot.generation_plan.final_profile
        };
        if audition {
            let used = state
                .takes
                .values()
                .filter(|take| take.shot_id == shot_id && take.profile == *profile)
                .count();
            let limit = usize::from(shot.generation_plan.audition_takes);
            if used >= limit {
                return failure(
                    request,
                    "AUDITION_LIMIT_REACHED",
                    format!(
                        "shot `{shot_id}` has used all {limit} audition take slots; select, reject, or revise the contract"
                    ),
                    false,
                    Some(state.revision),
                );
            }
        }
        let adapter_fingerprint = match self.adapter_fingerprint(shot.operation, profile) {
            Ok(fingerprint) => fingerprint,
            Err(error) => {
                return failure(
                    request,
                    error.code,
                    error.message,
                    error.retryable,
                    Some(state.revision),
                );
            }
        };
        if self.queue.revision == u64::MAX {
            return failure(
                request,
                "QUEUE_REVISION_OVERFLOW",
                "queue revision cannot be incremented".to_owned(),
                false,
                Some(state.revision),
            );
        }

        let now = timestamp();
        let job_id = format!("JOB-{}", Ulid::new());
        let reserved_take_id = format!("TAKE-{}", Ulid::new());
        let seed_hash =
            crate::store::sha256_bytes(format!("{}:{job_id}", request.command_id).as_bytes());
        let direct_seed = u64::from_str_radix(&seed_hash[..16], 16).unwrap_or(0);
        let selected_parent_id = if audition {
            None
        } else {
            state
                .shots
                .get(shot_id)
                .and_then(|runtime| runtime.selected_candidate_take_id.clone())
        };
        let (seed, parent_take_id, promotion_strategy) =
            if let Some(parent_take_id) = selected_parent_id {
                let Some(parent) = state.takes.get(&parent_take_id) else {
                    return failure(
                        request,
                        "TAKE_UNAVAILABLE",
                        format!("selected take `{parent_take_id}` is missing"),
                        false,
                        Some(state.revision),
                    );
                };
                if parent.stale || parent.shot_id != shot_id {
                    return failure(
                        request,
                        "TAKE_UNAVAILABLE",
                        format!("selected take `{parent_take_id}` is not usable for `{shot_id}`"),
                        false,
                        Some(state.revision),
                    );
                }
                (
                    parent.seed,
                    Some(parent_take_id),
                    Some(PromotionStrategy::SeedReplay),
                )
            } else {
                (direct_seed, None, None)
            };
        let input_hash = match sha256_json(&(
            state.active_contract_id.as_deref(),
            shot,
            profile,
            seed,
            parent_take_id.as_deref(),
            promotion_strategy,
        )) {
            Ok(hash) => hash,
            Err(error) => return store_failure(request, error),
        };
        let Some(contract_id) = state.active_contract_id.clone() else {
            return failure(
                request,
                "ACTIVE_CONTRACT_REQUIRED",
                "approve a ScriptBundle before queuing camera work".to_owned(),
                false,
                Some(state.revision),
            );
        };
        let job = JobJournal {
            schema_version: crate::domain::PROJECT_SCHEMA_VERSION.to_owned(),
            job_id: job_id.clone(),
            command_id: request.command_id.clone(),
            project_id: state.project_id.clone(),
            contract_id,
            shot_id: shot.id.clone(),
            reserved_take_id,
            operation: shot.operation,
            resolved_prompt: shot.prompt.clone(),
            seed,
            profile: profile.clone(),
            input_hash,
            adapter_fingerprint,
            parent_take_id,
            promotion_strategy,
            state: JobState::Queued,
            attempts: Vec::new(),
        };
        let audition_target_takes = audition.then_some(shot.generation_plan.audition_takes);
        let state = match store.enqueue_job_with_audition_target(
            &job,
            expected_revision,
            &request.command_id,
            &now,
            audition_target_takes,
        ) {
            Ok(state) => state,
            Err(error) => return store_failure(request, error),
        };

        let mut next_queue = self.queue.clone();
        next_queue.revision += 1;
        next_queue.last_command_id = Some(request.command_id.clone());
        next_queue.pending.push(QueueEntry {
            project_id: state.project_id.clone(),
            project_path: store.root().to_owned(),
            job_id: job_id.clone(),
            priority: "normal".to_owned(),
            resource: "gpu_exclusive".to_owned(),
        });
        self.queue = next_queue;
        if let Err(error) = write_json_atomic(&self.paths.queue_file(), &self.queue) {
            eprintln!(
                "queue snapshot write failed after durable job {job_id}; it will be rebuilt: {error}"
            );
        }
        match self.snapshot(&store, state) {
            Ok(snapshot) => success(
                request,
                Some(snapshot.revision),
                Some(snapshot),
                if audition {
                    "audition take queued"
                } else {
                    "final take queued"
                },
            ),
            Err(error) => worker_failure(request, error),
        }
    }

    fn mutate_take(
        &mut self,
        request: &ClientRequest,
        shot_id: &str,
        take_id: &str,
        mutation: TakeMutation,
    ) -> WorkerReply {
        let Some(expected_revision) = request.expected_revision else {
            return missing_revision(request);
        };
        let store = match self.project_store(request.project_id.as_deref()) {
            Ok(store) => store,
            Err(error) => return worker_failure(request, error),
        };
        let now = timestamp();
        let result = match mutation {
            TakeMutation::Select => store.select_take(
                shot_id,
                take_id,
                expected_revision,
                &request.command_id,
                &now,
            ),
            TakeMutation::Approve => store.approve_take(
                shot_id,
                take_id,
                expected_revision,
                &request.command_id,
                &now,
            ),
            TakeMutation::Reject => store.reject_take(
                shot_id,
                take_id,
                expected_revision,
                &request.command_id,
                &now,
            ),
        };
        match result {
            Ok(state) => match self.snapshot(&store, state) {
                Ok(snapshot) => success(
                    request,
                    Some(snapshot.revision),
                    Some(snapshot),
                    mutation.message(),
                ),
                Err(error) => worker_failure(request, error),
            },
            Err(error) => store_failure(request, error),
        }
    }

    fn retry_shot(&mut self, request: &ClientRequest, shot_id: &str) -> WorkerReply {
        let Some(expected_revision) = request.expected_revision else {
            return missing_revision(request);
        };
        let store = match self.project_store(request.project_id.as_deref()) {
            Ok(store) => store,
            Err(error) => return worker_failure(request, error),
        };
        let state = match store.read_state() {
            Ok(state) => state,
            Err(error) => return store_failure(request, error),
        };
        if state.revision != expected_revision {
            return store_failure(
                request,
                StoreError::RevisionConflict {
                    expected: expected_revision,
                    actual: state.revision,
                },
            );
        }
        let Some(shot) = state.shots.get(shot_id) else {
            return failure(
                request,
                "SHOT_NOT_FOUND",
                format!("shot `{shot_id}` is missing from the active contract"),
                false,
                Some(state.revision),
            );
        };
        if let Some(job_id) = &shot.active_job_id {
            return store_failure(
                request,
                StoreError::ShotBusy {
                    shot_id: shot_id.to_owned(),
                    job_id: job_id.clone(),
                },
            );
        }
        if shot.approved_take_id.is_some() {
            return store_failure(request, StoreError::ShotAlreadyApproved(shot_id.to_owned()));
        }
        if !matches!(shot.stage, ShotStage::Pending | ShotStage::Failed) {
            return failure(
                request,
                "SHOT_NOT_RETRYABLE",
                format!(
                    "shot `{shot_id}` is `{}`; retry requires pending or failed",
                    shot_stage(shot.stage)
                ),
                false,
                Some(state.revision),
            );
        }
        let audition = shot.selected_candidate_take_id.is_none();
        self.enqueue_shot(request, shot_id, audition)
    }

    fn preview_take(&self, request: &ClientRequest, take_id: &str) -> WorkerReply {
        let store = match self.project_store(request.project_id.as_deref()) {
            Ok(store) => store,
            Err(error) => return worker_failure(request, error),
        };
        let state = match store.read_state() {
            Ok(state) => state,
            Err(error) => return store_failure(request, error),
        };
        let Some(take) = state.takes.get(take_id) else {
            return store_failure(request, StoreError::TakeNotFound(take_id.to_owned()));
        };
        if take.media_path.is_absolute()
            || take
                .media_path
                .components()
                .any(|component| !matches!(component, Component::Normal(_) | Component::CurDir))
        {
            return failure(
                request,
                "ARTIFACT_PATH_INVALID",
                format!("take `{take_id}` has an unsafe media path"),
                false,
                Some(state.revision),
            );
        }
        let media_path = store.root().join(&take.media_path);
        if !media_path.is_file() {
            return failure(
                request,
                "ARTIFACT_NOT_FOUND",
                format!(
                    "take `{take_id}` media is missing at {}",
                    media_path.display()
                ),
                false,
                Some(state.revision),
            );
        }
        WorkerReply {
            protocol_version: IPC_PROTOCOL_VERSION.to_owned(),
            command_id: request.command_id.clone(),
            ok: true,
            revision: Some(state.revision),
            snapshot: None,
            artifact_path: Some(media_path),
            message: Some("take preview ready".to_owned()),
            error: None,
        }
    }
}
