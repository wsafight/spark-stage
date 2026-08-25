use super::*;

impl WorkerRuntime {
    pub(super) fn snapshot_for(
        &self,
        project_id: Option<&str>,
    ) -> Result<AppSnapshot, WorkerDomainError> {
        let store = self.project_store(project_id)?;
        let state = store.read_state()?;
        self.snapshot(&store, state)
    }

    pub(super) fn snapshot(
        &self,
        store: &ProjectStore,
        state: ProjectState,
    ) -> Result<AppSnapshot, WorkerDomainError> {
        let queue_jobs = self
            .queue
            .running
            .iter()
            .chain(self.queue.pending.iter())
            .filter(|entry| entry.project_id == state.project_id)
            .map(|entry| QueueJobSummary {
                job_id: entry.job_id.clone(),
                subject: entry.project_id.clone(),
                state: store.read_job(&entry.job_id).map_or_else(
                    |_| "unknown".to_owned(),
                    |job| match job.state {
                        JobState::Blocked => "blocked".to_owned(),
                        JobState::Active => "active".to_owned(),
                        JobState::Queued => "queued".to_owned(),
                        JobState::Completed => "completed".to_owned(),
                        JobState::Failed => "failed".to_owned(),
                        JobState::Cancelled => "cancelled".to_owned(),
                    },
                ),
                priority: entry.priority.clone(),
                resource: entry.resource.clone(),
                progress: None,
                eta_seconds: None,
            })
            .collect();
        let active_bundle = store.read_active_bundle()?;
        let shot_risks: HashMap<_, _> = active_bundle
            .as_ref()
            .map(|bundle| {
                bundle
                    .shots
                    .iter()
                    .map(|shot| (shot.id.as_str(), shot.generation_plan.risk))
                    .collect()
            })
            .unwrap_or_default();
        let audition_takes_limit = active_bundle.as_ref().map_or(0, |bundle| {
            bundle
                .shots
                .iter()
                .map(|shot| u32::from(shot.generation_plan.audition_takes))
                .sum()
        });
        let audition_takes_used = active_bundle.as_ref().map_or(0, |bundle| {
            let count = bundle
                .shots
                .iter()
                .map(|shot| {
                    state
                        .takes
                        .values()
                        .filter(|take| {
                            take.shot_id == shot.id
                                && take.profile == shot.generation_plan.audition_profile
                        })
                        .count()
                })
                .sum::<usize>();
            u32::try_from(count).unwrap_or(u32::MAX)
        });

        Ok(AppSnapshot {
            schema_version: crate::domain::PROJECT_SCHEMA_VERSION.to_owned(),
            revision: state.revision,
            refreshed_at: timestamp(),
            project: ProjectSummary {
                id: state.project_id.clone(),
                title: state.title.clone(),
                stage: project_stage(state.project_stage).to_owned(),
                outcome: project_outcome(state.project_outcome).to_owned(),
                work_mode: work_mode(state.work_mode).to_owned(),
                quality_target: quality_target(state.quality_target).to_owned(),
            },
            gpu: GpuSummary {
                status: if self.queue.running.is_some() {
                    "busy".to_owned()
                } else {
                    "idle".to_owned()
                },
                job_id: self
                    .queue
                    .running
                    .as_ref()
                    .map(|entry| entry.job_id.clone()),
                shot_id: None,
                progress: None,
                eta_seconds: None,
            },
            budget: BudgetSummary {
                audition_takes_used,
                audition_takes_limit,
                ..BudgetSummary::default()
            },
            pending_approvals: state
                .pending_approvals
                .iter()
                .map(|approval| ApprovalSummary {
                    approval_id: approval.approval_id.clone(),
                    kind: approval_kind(approval.kind).to_owned(),
                    shot_id: approval.shot_id.clone(),
                    take_ids: approval.take_ids.clone(),
                    blocking: approval.blocking,
                    description: approval.description.clone(),
                })
                .collect(),
            recent_failures: state
                .recent_failures
                .iter()
                .map(|failure| FailureSummary {
                    code: failure.code.clone(),
                    subject: failure.subject.clone(),
                    message: failure.message.clone(),
                    occurred_at: failure.occurred_at.clone(),
                })
                .collect(),
            shots: state
                .shots
                .values()
                .map(|shot| ShotSummary {
                    shot_id: shot.shot_id.clone(),
                    title: shot.title.clone(),
                    stage: shot_stage(shot.stage).to_owned(),
                    risk: risk(
                        shot_risks
                            .get(shot.shot_id.as_str())
                            .copied()
                            .unwrap_or(shot.risk),
                    )
                    .to_owned(),
                    candidate_count: shot.take_ids.len(),
                    selected_take_id: shot.selected_candidate_take_id.clone(),
                    approved_take_id: shot.approved_take_id.clone(),
                    fail_codes: shot.fail_codes.clone(),
                    stale: shot.stale,
                })
                .collect(),
            takes: state
                .takes
                .values()
                .map(|take| {
                    let shot = state.shots.get(&take.shot_id);
                    let selected = shot.and_then(|shot| shot.selected_candidate_take_id.as_deref())
                        == Some(take.take_id.as_str());
                    let approved = shot.and_then(|shot| shot.approved_take_id.as_deref())
                        == Some(take.take_id.as_str());
                    let rejected =
                        shot.is_some_and(|shot| shot.rejected_take_ids.contains(&take.take_id));
                    TakeSummary {
                        take_id: take.take_id.clone(),
                        shot_id: take.shot_id.clone(),
                        profile: take.profile.clone(),
                        status: if rejected {
                            "rejected"
                        } else if approved {
                            "approved"
                        } else if selected {
                            "selected"
                        } else {
                            &take.status
                        }
                        .to_owned(),
                        score: None,
                        hard_checks: take.hard_checks.clone(),
                        warnings: take.warnings.clone(),
                        selected,
                        approved,
                        media_path: Some(store.root().join(&take.media_path)),
                    }
                })
                .collect(),
            queue: QueueSummary {
                revision: self.queue.revision,
                paused: self.queue.paused,
                jobs: queue_jobs,
            },
            builds: state
                .builds
                .values()
                .map(|build| BuildSummary {
                    build_id: build.build_id.clone(),
                    kind: build.kind.clone(),
                    status: build.status.clone(),
                    recipe: build.recipe.clone(),
                    command_id: build.command_id.clone(),
                    output_path: build
                        .output_path
                        .as_ref()
                        .map(|path| store.root().join(path)),
                    warnings: build.warnings.clone(),
                    stale: build.stale,
                })
                .collect(),
            diagnostics: vec![DiagnosticSummary {
                probe_id: "worker".to_owned(),
                component: "worker".to_owned(),
                status: "ready".to_owned(),
                summary: match state.active_contract_id.as_ref() {
                    Some(contract_id) => format!("active contract {contract_id}"),
                    None => "waiting for approved script bundle".to_owned(),
                },
                capabilities: vec!["project_store".to_owned(), "script_authoring".to_owned()],
            }],
        })
    }

    pub(super) fn project_store(
        &self,
        project_id: Option<&str>,
    ) -> Result<ProjectStore, WorkerDomainError> {
        let project_id = match project_id {
            Some(project_id) => project_id.to_owned(),
            None => {
                let mut projects = fs::read_dir(&self.paths.projects_dir)
                    .map_err(|source| WorkerDomainError::Io {
                        path: self.paths.projects_dir.clone(),
                        source,
                    })?
                    .filter_map(Result::ok)
                    .filter(|entry| entry.path().join("project.json").is_file())
                    .filter_map(|entry| entry.file_name().into_string().ok())
                    .collect::<Vec<_>>();
                projects.sort();
                if projects.len() != 1 {
                    return Err(WorkerDomainError::ProjectRequired(projects));
                }
                projects.remove(0)
            }
        };
        ProjectStore::open(&self.paths.projects_dir, &project_id).map_err(Into::into)
    }

    pub(super) fn project_revision(&self, project_id: Option<&str>) -> Option<u64> {
        self.project_store(project_id)
            .and_then(|store| store.read_state().map_err(Into::into))
            .ok()
            .map(|state| state.revision)
    }
}
