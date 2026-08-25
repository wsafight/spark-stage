use super::super::*;

impl WorkerRuntime {
    pub(super) fn cancel_job(&mut self, request: &ClientRequest, job_id: &str) -> WorkerReply {
        let Some(expected_revision) = request.expected_revision else {
            return missing_revision(request);
        };
        let Some(next_queue_revision) = self.queue.revision.checked_add(1) else {
            return failure(
                request,
                "QUEUE_REVISION_OVERFLOW",
                "queue revision cannot be incremented".to_owned(),
                false,
                self.project_revision(request.project_id.as_deref()),
            );
        };
        if let Some(entry) = self
            .queue
            .running
            .as_ref()
            .filter(|entry| entry.job_id == job_id)
            .cloned()
        {
            if request.project_id.as_deref() != Some(entry.project_id.as_str()) {
                return failure(
                    request,
                    "JOB_PROJECT_MISMATCH",
                    format!("job `{job_id}` belongs to project `{}`", entry.project_id),
                    false,
                    self.project_revision(request.project_id.as_deref()),
                );
            }
            let store = match self.project_store(Some(&entry.project_id)) {
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
            let job = match store.read_job(job_id) {
                Ok(job) => job,
                Err(error) => return store_failure(request, error),
            };
            let interruptible = job.state == JobState::Active
                && job.attempts.last().is_some_and(|attempt| {
                    attempt.backend_job_id.is_some()
                        && matches!(
                            attempt.state,
                            AttemptState::Submitted | AttemptState::Running
                        )
                });
            if !interruptible {
                return failure(
                    request,
                    "JOB_CANCEL_NOT_READY",
                    format!("job `{job_id}` has not reached a confirmed ComfyUI backend execution"),
                    true,
                    Some(state.revision),
                );
            }
            let Some(adapter_config) = self.adapter_config.as_deref() else {
                return failure(
                    request,
                    "JOB_CANCEL_UNSUPPORTED",
                    "running-job cancellation requires an enabled adapter configuration".to_owned(),
                    false,
                    Some(state.revision),
                );
            };
            match interrupt_backend(adapter_config) {
                Ok(CancelOutcome::Interrupted) => {}
                Ok(CancelOutcome::Unsupported) => {
                    return failure(
                        request,
                        "JOB_CANCEL_UNSUPPORTED",
                        "adapter global interrupt is not explicitly enabled".to_owned(),
                        false,
                        Some(state.revision),
                    );
                }
                Err(message) => {
                    return failure(
                        request,
                        "JOB_CANCEL_FAILED",
                        message,
                        true,
                        Some(state.revision),
                    );
                }
            }
            let state = match store.cancel_running_job_after_interrupt(
                job_id,
                expected_revision,
                &request.command_id,
                &timestamp(),
            ) {
                Ok(state) => state,
                Err(error) => return store_failure(request, error),
            };
            let mut next_queue = self.queue.clone();
            next_queue.running = None;
            next_queue.revision = next_queue_revision;
            next_queue.last_command_id = Some(request.command_id.clone());
            if let Err(error) = write_json_atomic(&self.paths.queue_file(), &next_queue) {
                eprintln!(
                    "queue snapshot write failed after interrupting durable job {job_id}; it will be rebuilt: {error}"
                );
            }
            self.queue = next_queue;
            if let Some(cancellation) = &self.camera_cancellation {
                cancellation.request(job_id);
            }
            return match self.snapshot(&store, state) {
                Ok(snapshot) => success(
                    request,
                    Some(snapshot.revision),
                    Some(snapshot),
                    "running job interrupted and cancelled",
                ),
                Err(error) => worker_failure(request, error),
            };
        }
        let Some(entry) = self
            .queue
            .pending
            .iter()
            .find(|entry| entry.job_id == job_id)
            .cloned()
        else {
            return failure(
                request,
                "JOB_NOT_FOUND",
                format!("pending job `{job_id}` was not found"),
                false,
                self.project_revision(request.project_id.as_deref()),
            );
        };
        if request.project_id.as_deref() != Some(entry.project_id.as_str()) {
            return failure(
                request,
                "JOB_PROJECT_MISMATCH",
                format!("job `{job_id}` belongs to project `{}`", entry.project_id),
                false,
                self.project_revision(request.project_id.as_deref()),
            );
        }
        let store = match self.project_store(Some(&entry.project_id)) {
            Ok(store) => store,
            Err(error) => return worker_failure(request, error),
        };
        let state = match store.cancel_queued_job(
            job_id,
            expected_revision,
            &request.command_id,
            &timestamp(),
        ) {
            Ok(state) => state,
            Err(error) => return store_failure(request, error),
        };

        let mut next_queue = self.queue.clone();
        next_queue.pending.retain(|entry| entry.job_id != job_id);
        next_queue.revision = next_queue_revision;
        next_queue.last_command_id = Some(request.command_id.clone());
        if let Err(error) = write_json_atomic(&self.paths.queue_file(), &next_queue) {
            eprintln!(
                "queue snapshot write failed after cancelling durable job {job_id}; it will be rebuilt: {error}"
            );
        }
        self.queue = next_queue;
        match self.snapshot(&store, state) {
            Ok(snapshot) => success(
                request,
                Some(snapshot.revision),
                Some(snapshot),
                "pending job cancelled",
            ),
            Err(error) => worker_failure(request, error),
        }
    }
}

fn interrupt_backend(adapter_config: &Path) -> Result<CancelOutcome, String> {
    let config = ComfyAdapterConfig::load(adapter_config).map_err(|error| error.to_string())?;
    let adapter = ComfyAdapter::new(config).map_err(|error| error.to_string())?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| format!("cannot start cancellation runtime: {error}"))?;
    runtime
        .block_on(adapter.cancel(true))
        .map_err(|error| error.to_string())
}
