use super::*;

impl WorkerRuntime {
    pub(super) fn resume_auditions(&mut self) -> Result<(), WorkerRunError> {
        let entries =
            fs::read_dir(&self.paths.projects_dir).map_err(|source| WorkerRunError::Io {
                path: self.paths.projects_dir.clone(),
                source,
            })?;
        for entry in entries.filter_map(Result::ok) {
            let Some(project_id) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let Ok(store) = ProjectStore::open(&self.paths.projects_dir, &project_id) else {
                continue;
            };
            let state = store.read_state()?;
            let shot_ids = state
                .shots
                .values()
                .filter(|shot| shot.audition_target_takes.is_some() && shot.active_job_id.is_none())
                .map(|shot| shot.shot_id.clone())
                .collect::<Vec<_>>();
            for shot_id in shot_ids {
                self.schedule_next_audition(&store, &shot_id, None)?;
            }
        }
        Ok(())
    }

    pub(super) fn schedule_next_audition(
        &mut self,
        store: &ProjectStore,
        shot_id: &str,
        adapter_fingerprint: Option<&str>,
    ) -> Result<bool, WorkerRunError> {
        let mut state = store.read_state()?;
        let Some(runtime) = state.shots.get(shot_id) else {
            return Err(StoreError::ShotNotFound(shot_id.to_owned()).into());
        };
        let Some(target) = runtime.audition_target_takes else {
            return Ok(false);
        };
        if runtime.active_job_id.is_some()
            || runtime.selected_candidate_take_id.is_some()
            || runtime.approved_take_id.is_some()
        {
            return Ok(false);
        }
        let bundle = store
            .read_active_bundle()?
            .ok_or(StoreError::NoActiveContract)?;
        let shot = bundle
            .shots
            .iter()
            .find(|shot| shot.id == shot_id)
            .ok_or_else(|| StoreError::ShotNotFound(shot_id.to_owned()))?;
        let profile = &shot.generation_plan.audition_profile;
        let used = state
            .takes
            .values()
            .filter(|take| take.shot_id == shot_id && take.profile == *profile)
            .count();
        if used >= usize::from(target) {
            let expected_revision = state.revision;
            state
                .shots
                .get_mut(shot_id)
                .expect("shot was checked above")
                .audition_target_takes = None;
            state
                .bump_revision(timestamp())
                .map_err(StoreError::Invariant)?;
            store.save_state(&state, expected_revision)?;
            return Ok(false);
        }

        let adapter_fingerprint = adapter_fingerprint.map(str::to_owned).or_else(|| {
            state
                .takes
                .values()
                .filter(|take| take.shot_id == shot_id && take.profile == *profile && !take.stale)
                .max_by_key(|take| &take.take_id)
                .map(|take| take.adapter_fingerprint.clone())
        });
        let adapter_fingerprint = match adapter_fingerprint {
            Some(fingerprint) => fingerprint,
            None => self
                .adapter_fingerprint(shot.operation, profile)
                .map_err(|error| {
                    WorkerRunError::Recovery(format!(
                        "cannot resume audition for `{shot_id}`: {}",
                        error.message
                    ))
                })?,
        };
        let command_id = Ulid::new().to_string();
        let job_id = format!("JOB-{}", Ulid::new());
        let reserved_take_id = format!("TAKE-{}", Ulid::new());
        let seed_hash = crate::store::sha256_bytes(format!("{command_id}:{job_id}").as_bytes());
        let seed = u64::from_str_radix(&seed_hash[..16], 16).unwrap_or(0);
        let contract_id = state
            .active_contract_id
            .clone()
            .ok_or(StoreError::NoActiveContract)?;
        let reference_subjects = crate::store::reference_subject_keys(shot);
        let reference_fingerprint =
            crate::store::active_reference_fingerprint(&state, &reference_subjects);
        let input_hash = sha256_json(&(
            contract_id.as_str(),
            shot,
            profile,
            seed,
            Option::<&str>::None,
            Option::<PromotionStrategy>::None,
            reference_fingerprint,
        ))?;
        let job = JobJournal {
            schema_version: crate::domain::PROJECT_SCHEMA_VERSION.to_owned(),
            job_id: job_id.clone(),
            command_id: command_id.clone(),
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
            smoke_test: false,
            parent_take_id: None,
            promotion_strategy: None,
            state: JobState::Queued,
            attempts: Vec::new(),
        };
        let state = store.enqueue_job_with_audition_target(
            &job,
            state.revision,
            &command_id,
            &timestamp(),
            Some(target),
        )?;
        let next_revision = self
            .queue
            .revision
            .checked_add(1)
            .ok_or_else(|| WorkerRunError::Recovery("queue revision overflow".to_owned()))?;
        self.queue.revision = next_revision;
        self.queue.last_command_id = Some(command_id);
        self.queue.pending.push(QueueEntry {
            project_id: state.project_id,
            project_path: store.root().to_owned(),
            job_id,
            priority: "normal".to_owned(),
            resource: "gpu_exclusive".to_owned(),
        });
        if let Err(error) = write_json_atomic(&self.paths.queue_file(), &self.queue) {
            eprintln!(
                "queue snapshot write failed after auto-audition enqueue; it will be rebuilt: {error}"
            );
        }
        Ok(true)
    }
}
