use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{self, File};
use std::path::{Path, PathBuf};

use serde::Serialize;
use ulid::Ulid;

use super::{
    ExclusiveFileLock, StoreError, append_jsonl, io_error, read_json, sha256_bytes, sha256_json,
    write_json_atomic, write_text_atomic,
};
use crate::domain::{
    Approval, ApprovalKind, AttemptState, AuthoringReceipt, ContractRecord, ContractStatus,
    JobJournal, JobState, ProjectManifest, ProjectStage, ProjectState, ScriptBundle, ShotStage,
    TakeMetadata,
};

mod budget;
mod builds;
mod decisions;
mod references;
mod storage;
mod support;

pub use decisions::{BatchTakeSelection, DecisionRecord};
pub use references::{
    ReferenceImpact, ReferenceVerification, ReferenceWriteRequest, active_reference_fingerprint,
    reference_subject_keys,
};
pub use storage::*;
pub use support::validate_project_id;
use support::*;

#[derive(Debug, Clone)]
pub struct ProjectStore {
    root: PathBuf,
}

impl ProjectStore {
    pub fn create(
        projects_dir: &Path,
        project_id: &str,
        title: &str,
        brief: &str,
        command_id: &str,
        now: &str,
    ) -> Result<Self, StoreError> {
        validate_project_id(project_id)?;
        fs::create_dir_all(projects_dir).map_err(|source| io_error(projects_dir, source))?;
        let target = projects_dir.join(project_id);
        if target.exists() {
            return Err(StoreError::ProjectExists(project_id.to_owned()));
        }
        let staging = projects_dir.join(format!(".{project_id}.new-{}", Ulid::new()));
        let result: Result<(), StoreError> = (|| {
            for relative in [
                "contracts",
                "jobs",
                "raw",
                "review",
                "builds",
                "logs",
                "trash",
            ] {
                let directory = staging.join(relative);
                fs::create_dir_all(&directory).map_err(|source| io_error(&directory, source))?;
            }

            let manifest = ProjectManifest {
                schema_version: crate::domain::PROJECT_SCHEMA_VERSION.to_owned(),
                project_id: project_id.to_owned(),
                title: title.to_owned(),
                brief_hash: sha256_bytes(brief.as_bytes()),
                created_by_command_id: command_id.to_owned(),
                created_at: now.to_owned(),
            };
            let mut state =
                ProjectState::new(project_id.to_owned(), title.to_owned(), now.to_owned());
            state.last_command_id = Some(command_id.to_owned());
            write_json_atomic(&staging.join("project.json"), &manifest)?;
            write_text_atomic(&staging.join("script/brief.md"), brief)?;
            write_json_atomic(&staging.join("state.json"), &state)?;
            write_text_atomic(&staging.join("decisions.jsonl"), "")?;
            write_text_atomic(&staging.join("events.jsonl"), "")?;

            fs::rename(&staging, &target).map_err(|source| io_error(&target, source))?;
            File::open(projects_dir)
                .and_then(|directory| directory.sync_all())
                .map_err(|source| io_error(projects_dir, source))?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_dir_all(&staging);
        }
        result?;
        Ok(Self { root: target })
    }

    pub fn open(projects_dir: &Path, project_id: &str) -> Result<Self, StoreError> {
        validate_project_id(project_id)?;
        let root = projects_dir.join(project_id);
        if !root.is_dir() {
            return Err(StoreError::ProjectNotFound(project_id.to_owned()));
        }
        let store = Self { root };
        let manifest = store.read_manifest()?;
        if manifest.project_id != project_id {
            return Err(StoreError::ProjectIdMismatch {
                project: project_id.to_owned(),
                bundle: manifest.project_id,
            });
        }
        Ok(store)
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    #[must_use]
    pub fn state_path(&self) -> PathBuf {
        self.root.join("state.json")
    }

    pub fn lock(&self) -> Result<ExclusiveFileLock, StoreError> {
        ExclusiveFileLock::acquire(&self.root.join("project.lock"))
    }

    pub fn read_manifest(&self) -> Result<ProjectManifest, StoreError> {
        read_json(&self.root.join("project.json"))
    }

    pub fn read_state(&self) -> Result<ProjectState, StoreError> {
        let state: ProjectState = read_json(&self.state_path())?;
        state.validate()?;
        Ok(state)
    }

    pub fn save_state(
        &self,
        state: &ProjectState,
        expected_revision: u64,
    ) -> Result<(), StoreError> {
        let mut current = self.read_state()?;
        self.recover_cleanup_plans_for_state(&mut current)?;
        self.recover_decisions_for_state(&current)?;
        if current.revision != expected_revision {
            return Err(StoreError::RevisionConflict {
                expected: expected_revision,
                actual: current.revision,
            });
        }
        if state.revision != expected_revision.saturating_add(1) {
            return Err(StoreError::RevisionConflict {
                expected: expected_revision.saturating_add(1),
                actual: state.revision,
            });
        }
        state.validate()?;
        write_json_atomic(&self.state_path(), state)
    }

    pub fn apply_bundle(
        &self,
        bundle: &ScriptBundle,
        expected_revision: u64,
        command_id: &str,
        now: &str,
    ) -> Result<(ProjectState, Approval), StoreError> {
        let _lock = self.lock()?;
        let manifest = self.read_manifest()?;
        if manifest.project_id != bundle.project.id {
            return Err(StoreError::ProjectIdMismatch {
                project: manifest.project_id,
                bundle: bundle.project.id.clone(),
            });
        }
        let mut state = self.read_state()?;
        if state.revision != expected_revision {
            return Err(StoreError::RevisionConflict {
                expected: expected_revision,
                actual: state.revision,
            });
        }
        let bundle_hash = sha256_json(bundle)?;
        if let Some(record) = state.contracts.values().find(|record| {
            record.bundle_hash == bundle_hash && record.status == ContractStatus::PendingApproval
        }) {
            let approval = state
                .pending_approvals
                .iter()
                .find(|approval| approval.subject_id.as_deref() == Some(&record.contract_id))
                .cloned()
                .ok_or(StoreError::NoPendingScriptApproval)?;
            return Ok((state, approval));
        }

        let contract_id = format!("CON-{}", Ulid::new());
        let receipt_id = format!("RCP-{}", Ulid::new());
        let approval_id = format!("APR-{}", Ulid::new());
        let authoring = bundle.authoring.as_ref();
        let receipt = AuthoringReceipt {
            receipt_id,
            contract_id: contract_id.clone(),
            project_id: bundle.project.id.clone(),
            schema_version: bundle.schema_version.clone(),
            bundle_hash: bundle_hash.clone(),
            brief_hash: manifest.brief_hash,
            command_id: command_id.to_owned(),
            skill: authoring.map_or_else(|| "unspecified".to_owned(), |value| value.skill.clone()),
            agent_host: authoring.and_then(|value| value.agent_host.clone()),
            model: authoring.and_then(|value| value.model.clone()),
            created_at: now.to_owned(),
        };
        let relative_path = PathBuf::from("contracts").join(&contract_id);
        self.write_contract_version(&relative_path, bundle, &receipt)?;

        let approval = Approval {
            approval_id,
            kind: ApprovalKind::ScriptBundle,
            subject_id: Some(contract_id.clone()),
            shot_id: None,
            take_ids: Vec::new(),
            blocking: true,
            description: format!(
                "Review script bundle {} with {} shots",
                &bundle_hash[..12],
                bundle.shots.len()
            ),
            created_at: now.to_owned(),
        };
        state.contracts.insert(
            contract_id.clone(),
            ContractRecord {
                contract_id,
                relative_path,
                bundle_hash,
                status: ContractStatus::PendingApproval,
                receipt,
            },
        );
        state.pending_approvals.push(approval.clone());
        state.last_command_id = Some(command_id.to_owned());
        state.bump_revision(now.to_owned())?;
        self.save_state(&state, expected_revision)?;
        self.append_event(
            "script_bundle_applied",
            &approval.approval_id,
            command_id,
            now,
        )?;
        Ok((state, approval))
    }

    pub fn approve_script(
        &self,
        approval_id: Option<&str>,
        expected_revision: u64,
        command_id: &str,
        now: &str,
    ) -> Result<ProjectState, StoreError> {
        let _lock = self.lock()?;
        let mut state = self.read_state()?;
        if state.revision != expected_revision {
            return Err(StoreError::RevisionConflict {
                expected: expected_revision,
                actual: state.revision,
            });
        }
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
        if let Some(build) = state
            .builds
            .values()
            .find(|build| matches!(build.status.as_str(), "queued" | "running"))
        {
            return Err(StoreError::BuildBusy {
                build_id: build.build_id.clone(),
            });
        }
        let approval = state
            .pending_approvals
            .iter()
            .find(|approval| {
                approval.kind == ApprovalKind::ScriptBundle
                    && approval_id.is_none_or(|id| approval.approval_id == id)
            })
            .cloned()
            .ok_or(StoreError::NoPendingScriptApproval)?;
        let contract_id = approval
            .subject_id
            .clone()
            .ok_or(StoreError::NoPendingScriptApproval)?;
        let bundle = self.read_contract_bundle(&state, &contract_id)?;
        let previous_bundle = state
            .active_contract_id
            .as_deref()
            .and_then(|id| self.read_contract_bundle(&state, id).ok());
        let delivery_changed = previous_bundle
            .as_ref()
            .is_some_and(|previous| previous.project.delivery != bundle.project.delivery);

        let previous_shots: HashMap<_, _> = previous_bundle
            .as_ref()
            .map(|bundle| {
                bundle
                    .shots
                    .iter()
                    .map(|shot| (shot.id.as_str(), shot))
                    .collect()
            })
            .unwrap_or_default();
        let mut changed_shots = HashSet::new();
        let mut build_changed_shots = HashSet::new();
        let mut next_shots = BTreeMap::new();
        for shot in &bundle.shots {
            let previous_shot = previous_shots.get(shot.id.as_str()).copied();
            let bible_changed = previous_bundle
                .as_ref()
                .is_some_and(|previous| bible_change_affects_shot(previous, &bundle, shot));
            let generation_unchanged = previous_shot
                .is_some_and(|previous| shot_generation_equal(previous, shot))
                && !bible_changed;
            let build_unchanged =
                previous_shot.is_some_and(|previous| previous == shot) && !bible_changed;
            if !build_unchanged {
                build_changed_shots.insert(shot.id.clone());
            }
            let mut runtime = if generation_unchanged {
                state.shots.get(&shot.id).cloned().unwrap_or_else(|| {
                    initial_shot_state(
                        shot.id.clone(),
                        shot.title.clone(),
                        shot.generation_plan.risk,
                    )
                })
            } else {
                changed_shots.insert(shot.id.clone());
                initial_shot_state(
                    shot.id.clone(),
                    shot.title.clone(),
                    shot.generation_plan.risk,
                )
            };
            runtime.title.clone_from(&shot.title);
            runtime.risk = shot.generation_plan.risk;
            next_shots.insert(shot.id.clone(), runtime);
        }
        for old_id in state.shots.keys() {
            if !next_shots.contains_key(old_id) {
                changed_shots.insert(old_id.clone());
                build_changed_shots.insert(old_id.clone());
            }
        }
        for take in state.takes.values_mut() {
            if changed_shots.contains(&take.shot_id) {
                take.stale = true;
            }
        }
        self.mark_builds_stale_for_contract(&mut state, &build_changed_shots, delivery_changed);
        state.shots = next_shots;

        for (id, record) in &mut state.contracts {
            record.status = if id == &contract_id {
                ContractStatus::Active
            } else if record.status == ContractStatus::Active {
                ContractStatus::Superseded
            } else if record.status == ContractStatus::PendingApproval {
                ContractStatus::Rejected
            } else {
                record.status
            };
        }
        state.active_contract_id = Some(contract_id.clone());
        state.title.clone_from(&bundle.project.title);
        state.pending_approvals.retain(|pending| {
            pending.kind != ApprovalKind::ScriptBundle
                && (pending.kind != ApprovalKind::CandidateSelection
                    || pending
                        .shot_id
                        .as_ref()
                        .is_none_or(|shot_id| !changed_shots.contains(shot_id)))
        });
        state.project_stage = ProjectStage::Shooting;
        state.last_command_id = Some(command_id.to_owned());
        state.bump_revision(now.to_owned())?;
        let decision = self.prepare_decision(
            "script_bundle_approved",
            &approval.approval_id,
            command_id,
            now,
        )?;
        self.save_state(&state, expected_revision)?;
        self.commit_decisions(&[decision])?;
        self.write_active_contract_pointer(&state, &contract_id)?;
        Ok(state)
    }

    pub fn read_active_bundle(&self) -> Result<Option<ScriptBundle>, StoreError> {
        let state = self.read_state()?;
        state
            .active_contract_id
            .as_deref()
            .map(|contract_id| self.read_contract_bundle(&state, contract_id))
            .transpose()
    }

    pub fn read_contract_bundle_by_id(
        &self,
        contract_id: &str,
    ) -> Result<ScriptBundle, StoreError> {
        let state = self.read_state()?;
        self.read_contract_bundle(&state, contract_id)
    }

    pub fn enqueue_job(
        &self,
        job: &JobJournal,
        expected_revision: u64,
        command_id: &str,
        now: &str,
    ) -> Result<ProjectState, StoreError> {
        self.enqueue_job_with_audition_target(job, expected_revision, command_id, now, None)
    }

    pub fn enqueue_job_with_audition_target(
        &self,
        job: &JobJournal,
        expected_revision: u64,
        command_id: &str,
        now: &str,
        audition_target_takes: Option<u8>,
    ) -> Result<ProjectState, StoreError> {
        if audition_target_takes == Some(0) {
            return Err(StoreError::InvalidJob(
                "audition target must be greater than zero".to_owned(),
            ));
        }
        let _lock = self.lock()?;
        let mut state = self.read_state()?;
        if state.revision != expected_revision {
            return Err(StoreError::RevisionConflict {
                expected: expected_revision,
                actual: state.revision,
            });
        }
        let active_contract = state
            .active_contract_id
            .clone()
            .ok_or(StoreError::NoActiveContract)?;
        if job.contract_id != active_contract {
            return Err(StoreError::InvalidJob(
                "new job does not reference the active contract".to_owned(),
            ));
        }
        let bundle = self.read_contract_bundle(&state, &active_contract)?;
        if !bundle.shots.iter().any(|shot| shot.id == job.shot_id) {
            return Err(StoreError::ShotNotFound(job.shot_id.clone()));
        }
        match (&job.parent_take_id, job.promotion_strategy) {
            (Some(parent_take_id), Some(_)) => {
                let parent = state.takes.get(parent_take_id).ok_or_else(|| {
                    StoreError::InvalidJob(format!(
                        "promotion parent take `{parent_take_id}` does not exist"
                    ))
                })?;
                let shot = state
                    .shots
                    .get(&job.shot_id)
                    .ok_or_else(|| StoreError::ShotNotFound(job.shot_id.clone()))?;
                if parent.stale
                    || parent.shot_id != job.shot_id
                    || shot.selected_candidate_take_id.as_ref() != Some(parent_take_id)
                {
                    return Err(StoreError::InvalidJob(format!(
                        "promotion parent take `{parent_take_id}` is not the selected usable take"
                    )));
                }
            }
            (None, None) => {}
            _ => {
                return Err(StoreError::InvalidJob(
                    "promotion parent and strategy must either both be present or both be absent"
                        .to_owned(),
                ));
            }
        }
        let shot = state
            .shots
            .get_mut(&job.shot_id)
            .ok_or_else(|| StoreError::ShotNotFound(job.shot_id.clone()))?;
        if let Some(job_id) = &shot.active_job_id {
            return Err(StoreError::ShotBusy {
                shot_id: job.shot_id.clone(),
                job_id: job_id.clone(),
            });
        }
        validate_job(job, &state.project_id, command_id)?;
        let job_path = self.job_path(&job.job_id)?;
        if job_path.exists() {
            return Err(StoreError::InvalidJob(format!(
                "job `{}` already exists",
                job.job_id
            )));
        }

        write_json_atomic(&job_path, job)?;
        shot.stage = ShotStage::Queued;
        shot.active_job_id = Some(job.job_id.clone());
        shot.audition_target_takes = audition_target_takes;
        shot.fail_codes.clear();
        state.last_command_id = Some(command_id.to_owned());
        state.bump_revision(now.to_owned())?;
        if let Err(error) = self.save_state(&state, expected_revision) {
            let _ = fs::remove_file(&job_path);
            return Err(error);
        }
        Ok(state)
    }

    pub fn cancel_queued_job(
        &self,
        job_id: &str,
        expected_revision: u64,
        command_id: &str,
        now: &str,
    ) -> Result<ProjectState, StoreError> {
        let _lock = self.lock()?;
        let mut state = self.read_state()?;
        ensure_revision(&state, expected_revision)?;
        let mut job = self.read_job(job_id)?;
        if job.state != JobState::Queued {
            return Err(StoreError::JobNotCancellable {
                job_id: job_id.to_owned(),
                state: format!("{:?}", job.state).to_ascii_lowercase(),
            });
        }
        let shot = state
            .shots
            .get_mut(&job.shot_id)
            .ok_or_else(|| StoreError::ShotNotFound(job.shot_id.clone()))?;
        if shot.active_job_id.as_deref() != Some(job_id) {
            return Err(StoreError::InvalidJob(format!(
                "job `{job_id}` is not the active job for shot `{}`",
                job.shot_id
            )));
        }

        job.state = JobState::Cancelled;
        self.save_job(&job)?;
        shot.active_job_id = None;
        shot.audition_target_takes = None;
        let has_available_candidates = shot
            .take_ids
            .iter()
            .any(|take_id| !shot.rejected_take_ids.contains(take_id));
        shot.stage = if shot.selected_candidate_take_id.is_some() {
            ShotStage::Selected
        } else if has_available_candidates {
            ShotStage::CandidatesReady
        } else {
            ShotStage::Pending
        };
        state.last_command_id = Some(command_id.to_owned());
        state.bump_revision(now.to_owned())?;
        self.save_state(&state, expected_revision)?;
        self.append_event("job_cancelled", job_id, command_id, now)?;
        Ok(state)
    }

    pub fn cancel_running_job_after_interrupt(
        &self,
        job_id: &str,
        expected_revision: u64,
        command_id: &str,
        now: &str,
    ) -> Result<ProjectState, StoreError> {
        let _lock = self.lock()?;
        let mut state = self.read_state()?;
        ensure_revision(&state, expected_revision)?;
        let mut job = self.read_job(job_id)?;
        if job.state != JobState::Active {
            return Err(StoreError::JobNotCancellable {
                job_id: job_id.to_owned(),
                state: format!("{:?}", job.state).to_ascii_lowercase(),
            });
        }
        let shot = state
            .shots
            .get(&job.shot_id)
            .ok_or_else(|| StoreError::ShotNotFound(job.shot_id.clone()))?;
        if shot.active_job_id.as_deref() != Some(job_id) {
            return Err(StoreError::InvalidJob(format!(
                "job `{job_id}` is not the active job for shot `{}`",
                job.shot_id
            )));
        }
        let attempt = job.attempts.last_mut().ok_or_else(|| {
            StoreError::InvalidJob(format!("running job `{job_id}` has no active attempt"))
        })?;
        if attempt.backend_job_id.is_none()
            || !matches!(
                attempt.state,
                AttemptState::Submitted | AttemptState::Running
            )
        {
            return Err(StoreError::InvalidJob(format!(
                "running job `{job_id}` has not reached an interruptible backend state"
            )));
        }
        attempt.state = AttemptState::Cancelled;
        attempt.error_code = Some("USER_CANCELLED".to_owned());
        attempt.error_message = Some("ComfyUI confirmed global interrupt".to_owned());
        attempt.updated_at = now.to_owned();
        job.state = JobState::Cancelled;
        self.save_job(&job)?;

        let shot = state
            .shots
            .get_mut(&job.shot_id)
            .ok_or_else(|| StoreError::ShotNotFound(job.shot_id.clone()))?;
        shot.active_job_id = None;
        shot.audition_target_takes = None;
        let has_available_candidates = shot
            .take_ids
            .iter()
            .any(|take_id| !shot.rejected_take_ids.contains(take_id));
        shot.stage = if shot.selected_candidate_take_id.is_some() {
            ShotStage::Selected
        } else if has_available_candidates {
            ShotStage::CandidatesReady
        } else {
            ShotStage::Pending
        };
        state.last_command_id = Some(command_id.to_owned());
        state.bump_revision(now.to_owned())?;
        self.save_state(&state, expected_revision)?;
        self.append_event("running_job_interrupted", job_id, command_id, now)?;
        Ok(state)
    }

    pub fn read_job(&self, job_id: &str) -> Result<JobJournal, StoreError> {
        read_json(&self.job_path(job_id)?)
    }

    pub fn save_job(&self, job: &JobJournal) -> Result<(), StoreError> {
        validate_job_id(&job.job_id)?;
        write_json_atomic(
            &self.root.join("jobs").join(format!("{}.json", job.job_id)),
            job,
        )
    }

    pub fn save_take_metadata(&self, take: &TakeMetadata) -> Result<PathBuf, StoreError> {
        validate_take_id(&take.take_id)?;
        if take.shot_id.trim().is_empty() {
            return Err(StoreError::InvalidJob(
                "take shot id must not be empty".to_owned(),
            ));
        }
        let path = self
            .root
            .join("raw")
            .join(&take.shot_id)
            .join(format!("{}.json", take.take_id));
        if path.exists() {
            let existing: TakeMetadata = read_json(&path)?;
            if existing != *take {
                return Err(StoreError::InvalidJob(format!(
                    "immutable take `{}` already has different metadata",
                    take.take_id
                )));
            }
            return Ok(path);
        }
        write_json_atomic(&path, take)?;
        Ok(path)
    }

    pub fn read_take_metadata(
        &self,
        shot_id: &str,
        take_id: &str,
    ) -> Result<TakeMetadata, StoreError> {
        validate_take_id(take_id)?;
        read_json(
            &self
                .root
                .join("raw")
                .join(shot_id)
                .join(format!("{take_id}.json")),
        )
    }

    fn job_path(&self, job_id: &str) -> Result<PathBuf, StoreError> {
        validate_job_id(job_id)?;
        Ok(self.root.join("jobs").join(format!("{job_id}.json")))
    }

    fn read_contract_bundle(
        &self,
        state: &ProjectState,
        contract_id: &str,
    ) -> Result<ScriptBundle, StoreError> {
        let record = state
            .contracts
            .get(contract_id)
            .ok_or_else(|| StoreError::ContractNotFound(contract_id.to_owned()))?;
        read_json(
            &self
                .root
                .join(&record.relative_path)
                .join("script/bundle.json"),
        )
    }

    fn write_contract_version(
        &self,
        relative_path: &Path,
        bundle: &ScriptBundle,
        receipt: &AuthoringReceipt,
    ) -> Result<(), StoreError> {
        let target = self.root.join(relative_path);
        let staging = self
            .root
            .join("contracts")
            .join(format!(".{}.new", receipt.contract_id));
        let result: Result<(), StoreError> = (|| {
            render_contract(&staging, bundle, receipt)?;
            fs::rename(&staging, &target).map_err(|source| io_error(&target, source))?;
            File::open(self.root.join("contracts"))
                .and_then(|directory| directory.sync_all())
                .map_err(|source| io_error(self.root.join("contracts"), source))
        })();
        if result.is_err() {
            let _ = fs::remove_dir_all(&staging);
        }
        result
    }

    fn write_active_contract_pointer(
        &self,
        state: &ProjectState,
        contract_id: &str,
    ) -> Result<(), StoreError> {
        let record = state
            .contracts
            .get(contract_id)
            .ok_or_else(|| StoreError::ContractNotFound(contract_id.to_owned()))?;
        #[derive(Serialize)]
        struct Pointer<'a> {
            schema_version: &'a str,
            contract_id: &'a str,
            bundle_hash: &'a str,
            relative_path: &'a Path,
            state_revision: u64,
        }
        write_json_atomic(
            &self.root.join("active-contract.json"),
            &Pointer {
                schema_version: crate::domain::PROJECT_SCHEMA_VERSION,
                contract_id,
                bundle_hash: &record.bundle_hash,
                relative_path: &record.relative_path,
                state_revision: state.revision,
            },
        )
    }

    fn append_event(
        &self,
        kind: &str,
        subject_id: &str,
        command_id: &str,
        now: &str,
    ) -> Result<(), StoreError> {
        append_jsonl(
            &self.root.join("events.jsonl"),
            &JournalEntry {
                event_id: format!("EVT-{}", Ulid::new()),
                kind,
                subject_id,
                command_id,
                occurred_at: now,
            },
        )
    }
}

#[cfg(test)]
mod tests;
