use super::*;
use crate::domain::{BuildRecord, ProjectOutcome, QualityTarget};

fn valid_project_relative_path(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, std::path::Component::Normal(_)))
}

fn build_is_active(build: &BuildRecord) -> bool {
    matches!(build.status.as_str(), "queued" | "running")
}

fn mark_build_ids_stale(state: &mut ProjectState, stale_ids: &[String]) {
    for build_id in stale_ids {
        if let Some(build) = state.builds.get_mut(build_id) {
            build.stale = true;
        }
    }
    state.pending_approvals.retain(|approval| {
        !matches!(
            approval.kind,
            ApprovalKind::BuildReview | ApprovalKind::FinalVisualReview
        ) || approval
            .subject_id
            .as_ref()
            .is_none_or(|build_id| !stale_ids.contains(build_id))
    });
}

impl ProjectStore {
    pub(super) fn mark_builds_stale_for_decision(
        &self,
        state: &mut ProjectState,
        shot_id: &str,
        current_take_id: Option<&str>,
    ) {
        let stale_ids = state
            .builds
            .values()
            .filter(|build| !build.stale)
            .filter_map(|build| {
                let path = PathBuf::from(&build.recipe);
                if !valid_project_relative_path(&path) {
                    return Some(build.build_id.clone());
                }
                let recipe: crate::build::BuildRecipe =
                    match crate::store::read_json(&self.root.join(path)) {
                        Ok(recipe) => recipe,
                        Err(_) => return Some(build.build_id.clone()),
                    };
                recipe
                    .inputs
                    .iter()
                    .find(|input| input.shot_id == shot_id)
                    .is_some_and(|input| current_take_id != Some(input.take_id.as_str()))
                    .then(|| build.build_id.clone())
            })
            .collect::<Vec<_>>();
        mark_build_ids_stale(state, &stale_ids);
    }

    pub(super) fn mark_builds_stale_for_contract(
        &self,
        state: &mut ProjectState,
        changed_shots: &HashSet<String>,
        delivery_changed: bool,
    ) {
        let stale_ids = state
            .builds
            .values()
            .filter(|build| !build.stale)
            .filter_map(|build| {
                if delivery_changed {
                    return Some(build.build_id.clone());
                }
                let path = PathBuf::from(&build.recipe);
                if !valid_project_relative_path(&path) {
                    return Some(build.build_id.clone());
                }
                let recipe: crate::build::BuildRecipe =
                    match crate::store::read_json(&self.root.join(path)) {
                        Ok(recipe) => recipe,
                        Err(_) => return Some(build.build_id.clone()),
                    };
                recipe
                    .inputs
                    .iter()
                    .any(|input| changed_shots.contains(&input.shot_id))
                    .then(|| build.build_id.clone())
            })
            .collect::<Vec<_>>();
        mark_build_ids_stale(state, &stale_ids);
    }

    pub fn start_build(
        &self,
        build: BuildRecord,
        expected_revision: u64,
        command_id: &str,
        now: &str,
    ) -> Result<ProjectState, StoreError> {
        let _lock = self.lock()?;
        let mut state = self.read_state()?;
        ensure_revision(&state, expected_revision)?;
        if let Some(shot) = state
            .shots
            .values()
            .find(|shot| shot.active_job_id.is_some())
        {
            return Err(StoreError::ShotBusy {
                shot_id: shot.shot_id.clone(),
                job_id: shot.active_job_id.clone().unwrap_or_default(),
            });
        }
        if state.builds.contains_key(&build.build_id) {
            return Err(StoreError::InvalidJob(format!(
                "build `{}` already exists",
                build.build_id
            )));
        }
        if let Some(active) = state
            .builds
            .values()
            .find(|existing| build_is_active(existing))
        {
            return Err(StoreError::BuildBusy {
                build_id: active.build_id.clone(),
            });
        }
        if build.status != "queued" || build.output_path.is_some() || build.command_id != command_id
        {
            return Err(StoreError::InvalidJob(
                "new build must be queued without an output and owned by the command".to_owned(),
            ));
        }
        state.builds.insert(build.build_id.clone(), build);
        state.project_stage = ProjectStage::Build;
        state.last_command_id = Some(command_id.to_owned());
        state.bump_revision(now.to_owned())?;
        self.save_state(&state, expected_revision)?;
        Ok(state)
    }

    pub fn mark_build_running(
        &self,
        build_id: &str,
        expected_revision: u64,
        command_id: &str,
        now: &str,
    ) -> Result<ProjectState, StoreError> {
        let _lock = self.lock()?;
        let mut state = self.read_state()?;
        ensure_revision(&state, expected_revision)?;
        let build = state
            .builds
            .get_mut(build_id)
            .ok_or_else(|| StoreError::InvalidJob(format!("build `{build_id}` is missing")))?;
        if !build.command_id.is_empty() && build.command_id != command_id {
            return Err(StoreError::InvalidJob(format!(
                "build `{build_id}` is owned by a different command"
            )));
        }
        let command_id_backfilled = build.command_id.is_empty();
        if command_id_backfilled {
            build.command_id = command_id.to_owned();
        }
        if build.status == "running" {
            if command_id_backfilled {
                state.last_command_id = Some(command_id.to_owned());
                state.bump_revision(now.to_owned())?;
                self.save_state(&state, expected_revision)?;
            }
            return Ok(state);
        }
        if build.status != "queued" {
            return Err(StoreError::InvalidJob(format!(
                "build `{build_id}` cannot start from `{}`",
                build.status
            )));
        }
        build.status = "running".to_owned();
        state.last_command_id = Some(command_id.to_owned());
        state.bump_revision(now.to_owned())?;
        self.save_state(&state, expected_revision)?;
        self.append_event("build_started", build_id, command_id, now)?;
        Ok(state)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn finish_build(
        &self,
        build_id: &str,
        output_path: Option<PathBuf>,
        error: Option<String>,
        stale: bool,
        expected_revision: u64,
        command_id: &str,
        now: &str,
    ) -> Result<ProjectState, StoreError> {
        let _lock = self.lock()?;
        let mut state = self.read_state()?;
        ensure_revision(&state, expected_revision)?;
        let build = state
            .builds
            .get_mut(build_id)
            .ok_or_else(|| StoreError::InvalidJob(format!("build `{build_id}` is missing")))?;
        if !build_is_active(build) {
            return Err(StoreError::InvalidJob(format!(
                "build `{build_id}` is not active"
            )));
        }
        if let Some(message) = error {
            build.status = "failed".to_owned();
            build.warnings.push(message);
            build.stale = stale;
            state.project_stage = ProjectStage::Build;
        } else {
            let output_path = output_path.ok_or_else(|| {
                StoreError::InvalidJob("successful build has no output path".to_owned())
            })?;
            if !valid_project_relative_path(&output_path) {
                return Err(StoreError::InvalidJob(format!(
                    "successful build has unsafe output path `{}`",
                    output_path.display()
                )));
            }
            if !self.root.join(&output_path).is_file() {
                return Err(StoreError::InvalidJob(format!(
                    "successful build output is missing at `{}`",
                    output_path.display()
                )));
            }
            build.status = "needs_review".to_owned();
            build.output_path = Some(output_path);
            build.stale = false;
            let kind = if build.kind == "final" {
                ApprovalKind::FinalVisualReview
            } else {
                ApprovalKind::BuildReview
            };
            state.pending_approvals.retain(|approval| {
                !matches!(
                    approval.kind,
                    ApprovalKind::BuildReview | ApprovalKind::FinalVisualReview
                ) || approval.subject_id.as_deref() != Some(build_id)
            });
            state.pending_approvals.push(Approval {
                approval_id: format!("APR-{}", Ulid::new()),
                kind,
                subject_id: Some(build_id.to_owned()),
                shot_id: None,
                take_ids: Vec::new(),
                blocking: true,
                description: format!("Review {} build {build_id}", build.kind),
                created_at: now.to_owned(),
            });
            state.project_stage = ProjectStage::Review;
            state.quality_target = QualityTarget::DraftCut;
        }
        state.last_command_id = Some(command_id.to_owned());
        state.bump_revision(now.to_owned())?;
        self.save_state(&state, expected_revision)?;
        self.append_event("build_finished", build_id, command_id, now)?;
        Ok(state)
    }

    pub fn approve_build_review(
        &self,
        approval_id: &str,
        expected_revision: u64,
        command_id: &str,
        now: &str,
    ) -> Result<ProjectState, StoreError> {
        let _lock = self.lock()?;
        let mut state = self.read_state()?;
        ensure_revision(&state, expected_revision)?;
        let approval = state
            .pending_approvals
            .iter()
            .find(|approval| {
                approval.approval_id == approval_id
                    && matches!(
                        approval.kind,
                        ApprovalKind::BuildReview | ApprovalKind::FinalVisualReview
                    )
            })
            .cloned()
            .ok_or_else(|| StoreError::ApprovalNotFound(approval_id.to_owned()))?;
        let build_id = approval
            .subject_id
            .as_deref()
            .ok_or_else(|| StoreError::ApprovalNotFound(approval_id.to_owned()))?;
        let build = state
            .builds
            .get_mut(build_id)
            .ok_or_else(|| StoreError::InvalidJob(format!("build `{build_id}` is missing")))?;
        if build.status != "needs_review" {
            return Err(StoreError::InvalidJob(format!(
                "build `{build_id}` is not awaiting review"
            )));
        }
        if build.stale {
            return Err(StoreError::InvalidJob(format!(
                "build `{build_id}` is stale and must be rebuilt"
            )));
        }
        let output_path = build.output_path.as_ref().ok_or_else(|| {
            StoreError::InvalidJob(format!("build `{build_id}` has no review artifact"))
        })?;
        if !valid_project_relative_path(output_path) || !self.root.join(output_path).is_file() {
            return Err(StoreError::InvalidJob(format!(
                "build `{build_id}` review artifact is unavailable"
            )));
        }
        build.status = "approved".to_owned();
        state
            .pending_approvals
            .retain(|pending| pending.approval_id != approval_id);
        if approval.kind == ApprovalKind::FinalVisualReview {
            state.project_stage = ProjectStage::Completed;
            state.quality_target = QualityTarget::Playable;
            state.project_outcome = ProjectOutcome::Done;
        } else {
            state.project_stage = ProjectStage::Shooting;
            state.project_outcome = ProjectOutcome::InProgress;
        }
        state.last_command_id = Some(command_id.to_owned());
        state.bump_revision(now.to_owned())?;
        self.save_state(&state, expected_revision)?;
        self.append_decision("build_approved", build_id, command_id, now)?;
        Ok(state)
    }
}
