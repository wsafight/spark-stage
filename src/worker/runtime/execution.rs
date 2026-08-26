use super::*;

impl WorkerRuntime {
    pub(super) fn adapter_fingerprint(
        &self,
        operation: Operation,
        profile: &str,
    ) -> Result<String, GenerationGateError> {
        let path = self
            .adapter_config
            .as_ref()
            .ok_or_else(|| GenerationGateError {
                code: "ADAPTER_CONFIG_MISSING",
                message: "start the worker with --adapter-config after completing H3 preflight"
                    .to_owned(),
                retryable: false,
            })?;
        let config = ComfyAdapterConfig::load(path).map_err(|error| GenerationGateError {
            code: "ADAPTER_CONFIG_INVALID",
            message: error.to_string(),
            retryable: false,
        })?;
        if !config.enabled {
            return Err(GenerationGateError {
                code: "ADAPTER_DISABLED",
                message: "camera adapter is disabled".to_owned(),
                retryable: false,
            });
        }
        let operation_name = operation_name(operation);
        if !config
            .verified_operations
            .iter()
            .any(|verified| verified == operation_name)
        {
            return Err(GenerationGateError {
                code: "CAPABILITY_MISS",
                message: format!(
                    "operation `{operation_name}` is not recorded as smoke-tested for this workflow"
                ),
                retryable: false,
            });
        }
        if !config.profiles.contains_key(profile) {
            return Err(GenerationGateError {
                code: "PROFILE_NOT_FOUND",
                message: format!("profile `{profile}` is not configured by the adapter"),
                retryable: false,
            });
        }
        for required in operation_bindings(operation) {
            if !config.optional_bindings.contains_key(*required) {
                return Err(GenerationGateError {
                    code: "CAPABILITY_MISS",
                    message: format!("operation `{operation_name}` requires binding `{required}`"),
                    retryable: false,
                });
            }
        }
        let adapter = ComfyAdapter::new(config.clone()).map_err(|error| GenerationGateError {
            code: "ADAPTER_CONFIG_INVALID",
            message: error.to_string(),
            retryable: false,
        })?;
        let workflow_hash =
            adapter
                .validate_local_workflow()
                .map_err(|error| GenerationGateError {
                    code: "WORKFLOW_INVALID",
                    message: error.to_string(),
                    retryable: false,
                })?;
        sha256_json(&(config, workflow_hash)).map_err(|error| GenerationGateError {
            code: "ADAPTER_FINGERPRINT_FAILED",
            message: error.to_string(),
            retryable: false,
        })
    }

    pub(super) fn next_executor_request(
        &mut self,
    ) -> Result<Option<ExecutorRequest>, WorkerRunError> {
        if self.queue.paused {
            return Ok(None);
        }
        if self.queue.running.is_none() {
            let mut selected = None;
            for (index, entry) in self.queue.pending.iter().enumerate() {
                let store = ProjectStore::open(&self.paths.projects_dir, &entry.project_id)?;
                if !store.read_state()?.paused {
                    selected = Some((index, entry.clone()));
                    break;
                }
            }
            let Some((_index, entry)) = selected else {
                return Ok(None);
            };
            let store = ProjectStore::open(&self.paths.projects_dir, &entry.project_id)?;
            let mut job = store.read_job(&entry.job_id)?;
            if job.state != JobState::Queued {
                self.rebuild_queue_from_projects()?;
                return Ok(None);
            }
            let request_id = Ulid::new().to_string();
            let now = timestamp();
            job.state = JobState::Active;
            job.attempts.push(AttemptJournal {
                request_id,
                client_id: None,
                workflow_hash: None,
                backend_job_id: None,
                state: AttemptState::Prepared,
                created_at: now.clone(),
                updated_at: now.clone(),
                error_code: None,
                error_message: None,
                output_path: None,
            });
            store.save_job(&job)?;
            let mut state = store.read_state()?;
            let expected_revision = state.revision;
            if let Some(shot) = state.shots.get_mut(&job.shot_id) {
                shot.stage = ShotStage::Generating;
            }
            state.bump_revision(now).map_err(StoreError::Invariant)?;
            store.save_state(&state, expected_revision)?;
            let mut queue = self.queue.clone();
            queue.running = Some(entry.clone());
            queue
                .pending
                .retain(|pending| pending.job_id != entry.job_id);
            queue.revision = queue
                .revision
                .checked_add(1)
                .ok_or_else(|| WorkerRunError::Recovery("queue revision overflow".to_owned()))?;
            write_json_atomic(&self.paths.queue_file(), &queue)?;
            self.queue = queue;
            return match self.execution_context(&entry, job) {
                Ok(context) => Ok(Some(ExecutorRequest::Prepare(context))),
                Err(error) => {
                    self.block_dispatch(&entry, &error.to_string())?;
                    Ok(None)
                }
            };
        }

        let entry = self.queue.running.clone().ok_or_else(|| {
            WorkerRunError::Recovery("running queue entry disappeared".to_owned())
        })?;
        let store = ProjectStore::open(&self.paths.projects_dir, &entry.project_id)?;
        let mut job = store.read_job(&entry.job_id)?;
        if job.state == JobState::Blocked {
            return Ok(None);
        }
        let Some(attempt) = job.attempts.last().cloned() else {
            return Err(WorkerRunError::Recovery(format!(
                "active job `{}` has no attempt",
                job.job_id
            )));
        };
        let context = match self.execution_context(&entry, job.clone()) {
            Ok(context) => context,
            Err(error) => {
                self.block_dispatch(&entry, &error.to_string())?;
                return Ok(None);
            }
        };
        match attempt.state {
            AttemptState::Prepared => Ok(Some(ExecutorRequest::Prepare(context))),
            AttemptState::Submitting if attempt.backend_job_id.is_none() => {
                let current = job
                    .attempts
                    .last_mut()
                    .ok_or_else(|| WorkerRunError::Recovery("attempt disappeared".to_owned()))?;
                current.state = AttemptState::SubmissionUnknown;
                current.error_code = Some("SUBMISSION_UNKNOWN".to_owned());
                current.error_message = Some(
                    "worker restarted after submission began but before backend id was recorded"
                        .to_owned(),
                );
                current.updated_at = timestamp();
                job.state = JobState::Blocked;
                store.save_job(&job)?;
                self.touch_queue_revision()?;
                Ok(None)
            }
            AttemptState::Submitted | AttemptState::Running => {
                let backend_job_id = attempt.backend_job_id.ok_or_else(|| {
                    WorkerRunError::Recovery("submitted attempt has no backend job id".to_owned())
                })?;
                let client_id = attempt.client_id.ok_or_else(|| {
                    WorkerRunError::Recovery("submitted attempt has no client id".to_owned())
                })?;
                let workflow_hash = attempt.workflow_hash.ok_or_else(|| {
                    WorkerRunError::Recovery("submitted attempt has no workflow hash".to_owned())
                })?;
                Ok(Some(ExecutorRequest::Reconcile {
                    context,
                    request_id: attempt.request_id,
                    client_id,
                    workflow_hash,
                    backend_job_id: crate::adapter::BackendJobId(backend_job_id),
                }))
            }
            _ => Ok(None),
        }
    }

    fn execution_context(
        &self,
        entry: &QueueEntry,
        job: JobJournal,
    ) -> Result<ExecutionContext, WorkerRunError> {
        let adapter_config = self.adapter_config.clone().ok_or_else(|| {
            WorkerRunError::Recovery("queued job has no adapter config".to_owned())
        })?;
        let actual_fingerprint = self
            .adapter_fingerprint(job.operation, &job.profile)
            .map_err(|error| {
                WorkerRunError::Recovery(format!(
                    "queued job `{}` cannot use the current adapter: {}",
                    job.job_id, error.message
                ))
            })?;
        if actual_fingerprint != job.adapter_fingerprint {
            return Err(WorkerRunError::Recovery(format!(
                "queued job `{}` adapter or workflow fingerprint changed",
                job.job_id
            )));
        }
        let store = ProjectStore::open(&self.paths.projects_dir, &entry.project_id)?;
        let bundle = store.read_contract_bundle_by_id(&job.contract_id)?;
        let shot = bundle
            .shots
            .into_iter()
            .find(|shot| shot.id == job.shot_id)
            .ok_or_else(|| {
                WorkerRunError::Recovery(format!(
                    "shot `{}` is missing from the active contract",
                    job.shot_id
                ))
            })?;
        Ok(ExecutionContext {
            adapter_config,
            project_root: store.root().to_owned(),
            job,
            shot,
        })
    }

    fn block_dispatch(&mut self, entry: &QueueEntry, message: &str) -> Result<(), WorkerRunError> {
        let store = ProjectStore::open(&self.paths.projects_dir, &entry.project_id)?;
        let mut job = store.read_job(&entry.job_id)?;
        let now = timestamp();
        let attempt = job.attempts.last_mut().ok_or_else(|| {
            WorkerRunError::Recovery(format!(
                "active job `{}` has no attempt to block",
                job.job_id
            ))
        })?;
        attempt.error_code = Some("DISPATCH_BLOCKED".to_owned());
        attempt.error_message = Some(message.to_owned());
        attempt.updated_at = now.clone();
        job.state = JobState::Blocked;
        store.save_job(&job)?;

        let mut state = store.read_state()?;
        let expected_revision = state.revision;
        let failure = FailureRecord {
            code: "DISPATCH_BLOCKED".to_owned(),
            subject: job.shot_id.clone(),
            message: message.to_owned(),
            occurred_at: now.clone(),
        };
        if let Some(shot) = state.shots.get_mut(&job.shot_id)
            && !shot.fail_codes.contains(&failure.code)
        {
            shot.fail_codes.push(failure.code.clone());
        }
        state.recent_failures.push(failure);
        state.bump_revision(now).map_err(StoreError::Invariant)?;
        store.save_state(&state, expected_revision)?;
        self.touch_queue_revision()?;
        self.emit_milestone(
            MilestoneKind::CameraFailed,
            entry.project_id.clone(),
            entry.job_id.clone(),
            message.to_owned(),
        );
        Ok(())
    }

    pub(super) fn apply_executor_event(
        &mut self,
        event: ExecutorEvent,
    ) -> Result<Option<ExecutorRequest>, WorkerRunError> {
        let project_id = self
            .queue
            .running
            .as_ref()
            .map(|entry| entry.project_id.clone())
            .unwrap_or_default();
        let milestone = match &event {
            ExecutorEvent::Completed { job_id, .. } => Some((
                MilestoneKind::TakeReady,
                job_id.clone(),
                "camera take is ready for review".to_owned(),
            )),
            ExecutorEvent::OutputInvalid {
                job_id,
                code,
                message,
                ..
            }
            | ExecutorEvent::PreparationFailed {
                job_id,
                code,
                message,
                ..
            } => Some((
                MilestoneKind::CameraFailed,
                job_id.clone(),
                format!("{code}: {message}"),
            )),
            ExecutorEvent::BackendFailed {
                job_id, message, ..
            }
            | ExecutorEvent::SubmissionUnknown {
                job_id, message, ..
            } => Some((MilestoneKind::CameraFailed, job_id.clone(), message.clone())),
            _ => None,
        };
        let changes_state = !matches!(&event, ExecutorEvent::Cancelled { .. });
        let queue_revision = self.queue.revision;
        let result: Result<Option<ExecutorRequest>, WorkerRunError> = match event {
            ExecutorEvent::Prepared {
                mut context,
                prepared,
            } => {
                let prepared = *prepared;
                let store = ProjectStore::open(&self.paths.projects_dir, &context.job.project_id)?;
                let mut job = store.read_job(&context.job.job_id)?;
                let attempt = matching_attempt_mut(&mut job, &prepared.request_id)?;
                attempt.client_id = Some(prepared.client_id.clone());
                attempt.workflow_hash = Some(prepared.workflow_hash.clone());
                attempt.state = AttemptState::Submitting;
                attempt.updated_at = timestamp();
                store.save_job(&job)?;
                context.job = job;
                Ok(Some(ExecutorRequest::Submit {
                    context: *context,
                    prepared,
                }))
            }
            ExecutorEvent::Submitted {
                job_id,
                request_id,
                backend_job_id,
            } => {
                let store = self.running_store(&job_id)?;
                let mut job = store.read_job(&job_id)?;
                let attempt = matching_attempt_mut(&mut job, &request_id)?;
                attempt.backend_job_id = Some(backend_job_id.0);
                attempt.state = AttemptState::Submitted;
                attempt.updated_at = timestamp();
                store.save_job(&job)?;
                Ok(None)
            }
            ExecutorEvent::Completed {
                job_id,
                request_id,
                workflow_hash,
                model_fingerprint,
                media_path,
                report,
                boundaries,
                elapsed_milliseconds,
            } => {
                self.complete_job(
                    &job_id,
                    &request_id,
                    &workflow_hash,
                    &model_fingerprint,
                    &media_path,
                    &report,
                    &boundaries,
                    elapsed_milliseconds,
                )?;
                Ok(None)
            }
            ExecutorEvent::OutputInvalid {
                job_id,
                request_id,
                code,
                message,
                staging_path,
                report,
            } => {
                let detail = match (staging_path, report) {
                    (Some(path), Some(report)) => format!(
                        "{message}; retained {}; {} checks failed",
                        path.display(),
                        report
                            .checks
                            .iter()
                            .filter(|check| check.status == crate::media::MediaCheckStatus::Fail)
                            .count()
                    ),
                    (Some(path), None) => {
                        format!("{message}; retained {}", path.display())
                    }
                    _ => message,
                };
                self.fail_job(
                    &job_id,
                    &request_id,
                    &code,
                    &detail,
                    AttemptState::OutputInvalid,
                )?;
                Ok(None)
            }
            ExecutorEvent::BackendFailed {
                job_id,
                request_id,
                message,
            } => {
                self.fail_job(
                    &job_id,
                    &request_id,
                    "BACKEND_FAILED",
                    &message,
                    AttemptState::BackendFailed,
                )?;
                Ok(None)
            }
            ExecutorEvent::PreparationFailed {
                job_id,
                request_id,
                code,
                message,
            } => {
                self.fail_job(
                    &job_id,
                    &request_id,
                    &code,
                    &message,
                    AttemptState::BackendFailed,
                )?;
                Ok(None)
            }
            ExecutorEvent::SubmissionUnknown {
                job_id,
                request_id,
                message,
            } => {
                let store = self.running_store(&job_id)?;
                let mut job = store.read_job(&job_id)?;
                let attempt = matching_attempt_mut(&mut job, &request_id)?;
                attempt.state = AttemptState::SubmissionUnknown;
                attempt.error_code = Some("SUBMISSION_UNKNOWN".to_owned());
                attempt.error_message = Some(message);
                attempt.updated_at = timestamp();
                job.state = JobState::Blocked;
                store.save_job(&job)?;
                Ok(None)
            }
            ExecutorEvent::RetryableMonitorError {
                job_id,
                request_id,
                message,
            } => {
                let store = self.running_store(&job_id)?;
                let mut job = store.read_job(&job_id)?;
                let attempt = matching_attempt_mut(&mut job, &request_id)?;
                attempt.state = AttemptState::Running;
                attempt.error_code = Some("MONITOR_RETRY".to_owned());
                attempt.error_message = Some(message);
                attempt.updated_at = timestamp();
                store.save_job(&job)?;
                Ok(None)
            }
            ExecutorEvent::Cancelled { job_id, request_id } => {
                let _cancelled_request = (job_id, request_id);
                Ok(None)
            }
        };
        let next_request = result?;
        if changes_state && self.queue.revision == queue_revision {
            self.touch_queue_revision()?;
        }
        if let Some((kind, subject_id, message)) = milestone {
            self.emit_milestone(kind, project_id, subject_id, message);
        }
        Ok(next_request)
    }

    #[allow(clippy::too_many_arguments)]
    fn complete_job(
        &mut self,
        job_id: &str,
        request_id: &str,
        workflow_hash: &str,
        model_fingerprint: &str,
        media_path: &Path,
        report: &crate::media::MediaReport,
        boundaries: &crate::media::BoundaryFrames,
        elapsed_milliseconds: u64,
    ) -> Result<(), WorkerRunError> {
        let store = self.running_store(job_id)?;
        let mut job = store.read_job(job_id)?;
        let relative_media = relative_project_path(store.root(), media_path)?;
        let take = TakeMetadata {
            take_id: job.reserved_take_id.clone(),
            shot_id: job.shot_id.clone(),
            job_id: job.job_id.clone(),
            profile: job.profile.clone(),
            status: "candidate".to_owned(),
            media_path: relative_media.clone(),
            input_hash: job.input_hash.clone(),
            adapter_fingerprint: job.adapter_fingerprint.clone(),
            workflow_hash: workflow_hash.to_owned(),
            model_fingerprint: model_fingerprint.to_owned(),
            seed: job.seed,
            elapsed_milliseconds,
            first_frame_path: Some(relative_project_path(store.root(), &boundaries.first)?),
            last_frame_path: Some(relative_project_path(store.root(), &boundaries.last)?),
            handoff_candidate_path: Some(relative_project_path(
                store.root(),
                &boundaries.handoff_candidate,
            )?),
            parent_take_id: job.parent_take_id.clone(),
            promotion_strategy: job.promotion_strategy,
            hard_checks: report
                .checks
                .iter()
                .map(|check| check.code.clone())
                .collect(),
            warnings: if job.promotion_strategy == Some(PromotionStrategy::SeedReplay) {
                vec!["seed replay is a new take and does not preserve audition pixels".to_owned()]
            } else {
                Vec::new()
            },
            stale: false,
        };
        store.save_take_metadata(&take)?;
        let attempt = matching_attempt_mut(&mut job, request_id)?;
        attempt.state = AttemptState::Completed;
        attempt.output_path = Some(relative_media);
        attempt.error_code = None;
        attempt.error_message = None;
        attempt.updated_at = timestamp();
        job.state = JobState::Completed;
        store.save_job(&job)?;

        let mut state = store.read_state()?;
        let expected_revision = state.revision;
        register_candidate(&mut state, &take, &job.shot_id, &timestamp());
        state
            .bump_revision(timestamp())
            .map_err(StoreError::Invariant)?;
        store.save_state(&state, expected_revision)?;
        self.finish_running_queue(job_id)?;
        self.schedule_next_audition(&store, &job.shot_id, Some(&job.adapter_fingerprint))?;
        Ok(())
    }

    fn fail_job(
        &mut self,
        job_id: &str,
        request_id: &str,
        code: &str,
        message: &str,
        attempt_state: AttemptState,
    ) -> Result<(), WorkerRunError> {
        let store = self.running_store(job_id)?;
        let mut job = store.read_job(job_id)?;
        let attempt = matching_attempt_mut(&mut job, request_id)?;
        attempt.state = attempt_state;
        attempt.error_code = Some(code.to_owned());
        attempt.error_message = Some(message.to_owned());
        attempt.updated_at = timestamp();
        job.state = JobState::Failed;
        store.save_job(&job)?;
        let mut state = store.read_state()?;
        let expected_revision = state.revision;
        if let Some(shot) = state.shots.get_mut(&job.shot_id) {
            shot.stage = ShotStage::Failed;
            shot.active_job_id = None;
            shot.audition_target_takes = None;
            if !shot.fail_codes.iter().any(|existing| existing == code) {
                shot.fail_codes.push(code.to_owned());
            }
        }
        state.recent_failures.push(FailureRecord {
            code: code.to_owned(),
            subject: job.shot_id,
            message: message.to_owned(),
            occurred_at: timestamp(),
        });
        state
            .bump_revision(timestamp())
            .map_err(StoreError::Invariant)?;
        store.save_state(&state, expected_revision)?;
        self.finish_running_queue(job_id)
    }

    fn running_store(&self, job_id: &str) -> Result<ProjectStore, WorkerRunError> {
        let entry = self
            .queue
            .running
            .as_ref()
            .filter(|entry| entry.job_id == job_id)
            .ok_or_else(|| {
                WorkerRunError::Recovery(format!("job `{job_id}` is not the running queue entry"))
            })?;
        ProjectStore::open(&self.paths.projects_dir, &entry.project_id).map_err(Into::into)
    }

    fn finish_running_queue(&mut self, job_id: &str) -> Result<(), WorkerRunError> {
        if self
            .queue
            .running
            .as_ref()
            .is_none_or(|entry| entry.job_id != job_id)
        {
            return Err(WorkerRunError::Recovery(format!(
                "job `{job_id}` is not running"
            )));
        }
        self.queue.running = None;
        self.queue.revision = self
            .queue
            .revision
            .checked_add(1)
            .ok_or_else(|| WorkerRunError::Recovery("queue revision overflow".to_owned()))?;
        write_json_atomic(&self.paths.queue_file(), &self.queue)?;
        Ok(())
    }
}
