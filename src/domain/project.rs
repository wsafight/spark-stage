use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::{Operation, Risk};

pub const PROJECT_SCHEMA_VERSION: &str = "1.0";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectManifest {
    pub schema_version: String,
    pub project_id: String,
    pub title: String,
    pub brief_hash: String,
    pub created_by_command_id: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectStage {
    Authoring,
    Shooting,
    Review,
    Build,
    Completed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectOutcome {
    InProgress,
    NeedsReview,
    Done,
    DoneWithWarnings,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkMode {
    Fast,
    Director,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QualityTarget {
    DraftCut,
    Playable,
    Approved,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectState {
    pub schema_version: String,
    pub revision: u64,
    pub project_id: String,
    pub title: String,
    pub project_stage: ProjectStage,
    pub project_outcome: ProjectOutcome,
    pub work_mode: WorkMode,
    pub quality_target: QualityTarget,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_command_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_contract_id: Option<String>,
    #[serde(default)]
    pub contracts: BTreeMap<String, ContractRecord>,
    #[serde(default)]
    pub pending_approvals: Vec<Approval>,
    #[serde(default)]
    pub shots: BTreeMap<String, ShotRuntimeState>,
    #[serde(default)]
    pub takes: BTreeMap<String, TakeMetadata>,
    #[serde(default)]
    pub builds: BTreeMap<String, BuildRecord>,
    #[serde(default)]
    pub recent_failures: Vec<FailureRecord>,
    pub created_at: String,
    pub updated_at: String,
}

impl ProjectState {
    #[must_use]
    pub fn new(project_id: String, title: String, now: String) -> Self {
        Self {
            schema_version: PROJECT_SCHEMA_VERSION.to_owned(),
            revision: 1,
            project_id,
            title,
            project_stage: ProjectStage::Authoring,
            project_outcome: ProjectOutcome::InProgress,
            work_mode: WorkMode::Director,
            quality_target: QualityTarget::Playable,
            last_command_id: None,
            active_contract_id: None,
            contracts: BTreeMap::new(),
            pending_approvals: Vec::new(),
            shots: BTreeMap::new(),
            takes: BTreeMap::new(),
            builds: BTreeMap::new(),
            recent_failures: Vec::new(),
            created_at: now.clone(),
            updated_at: now,
        }
    }

    pub fn bump_revision(&mut self, now: String) -> Result<(), StateInvariantError> {
        self.revision = self
            .revision
            .checked_add(1)
            .ok_or(StateInvariantError::RevisionOverflow)?;
        self.updated_at = now;
        self.refresh_derived_outcome();
        self.validate()
    }

    pub fn refresh_derived_outcome(&mut self) {
        if self
            .pending_approvals
            .iter()
            .any(|approval| approval.blocking)
        {
            self.project_outcome = ProjectOutcome::NeedsReview;
        } else if self.project_outcome == ProjectOutcome::NeedsReview {
            self.project_outcome = ProjectOutcome::InProgress;
        }
    }

    pub fn validate(&self) -> Result<(), StateInvariantError> {
        if self.schema_version != PROJECT_SCHEMA_VERSION {
            return Err(StateInvariantError::UnsupportedSchema(
                self.schema_version.clone(),
            ));
        }
        if self.revision == 0 {
            return Err(StateInvariantError::ZeroRevision);
        }
        if self
            .active_contract_id
            .as_ref()
            .is_some_and(|id| !self.contracts.contains_key(id))
        {
            return Err(StateInvariantError::MissingActiveContract);
        }
        if self
            .pending_approvals
            .iter()
            .any(|approval| approval.blocking)
            != (self.project_outcome == ProjectOutcome::NeedsReview)
            && !matches!(
                self.project_outcome,
                ProjectOutcome::Failed | ProjectOutcome::Cancelled
            )
        {
            return Err(StateInvariantError::OutcomeApprovalMismatch);
        }

        let mut approval_ids = HashSet::new();
        if let Some(duplicate) = self
            .pending_approvals
            .iter()
            .find(|approval| !approval_ids.insert(approval.approval_id.as_str()))
        {
            return Err(StateInvariantError::DuplicateApproval(
                duplicate.approval_id.clone(),
            ));
        }

        for (key, take) in &self.takes {
            if key != &take.take_id {
                return Err(StateInvariantError::TakeKeyMismatch {
                    key: key.clone(),
                    take_id: take.take_id.clone(),
                });
            }
        }
        for (key, shot) in &self.shots {
            if key != &shot.shot_id {
                return Err(StateInvariantError::ShotKeyMismatch {
                    key: key.clone(),
                    shot_id: shot.shot_id.clone(),
                });
            }
            let mut take_ids = HashSet::new();
            for take_id in &shot.take_ids {
                if !take_ids.insert(take_id.as_str()) {
                    return Err(StateInvariantError::DuplicateShotTake {
                        shot_id: shot.shot_id.clone(),
                        take_id: take_id.clone(),
                    });
                }
                let take = self.takes.get(take_id).ok_or_else(|| {
                    StateInvariantError::ShotTakeMissing {
                        shot_id: shot.shot_id.clone(),
                        take_id: take_id.clone(),
                    }
                })?;
                if take.shot_id != shot.shot_id {
                    return Err(StateInvariantError::TakeShotMismatch {
                        take_id: take_id.clone(),
                        expected_shot_id: shot.shot_id.clone(),
                        actual_shot_id: take.shot_id.clone(),
                    });
                }
            }
            for take_id in &shot.rejected_take_ids {
                if !take_ids.contains(take_id.as_str()) {
                    return Err(StateInvariantError::RejectedTakeUnavailable {
                        shot_id: shot.shot_id.clone(),
                        take_id: take_id.clone(),
                    });
                }
            }
            for (kind, take_id) in [
                ("selected", shot.selected_candidate_take_id.as_ref()),
                ("approved", shot.approved_take_id.as_ref()),
            ] {
                if let Some(take_id) = take_id
                    && (!take_ids.contains(take_id.as_str())
                        || shot.rejected_take_ids.contains(take_id)
                        || self.takes.get(take_id).is_some_and(|take| take.stale))
                {
                    return Err(StateInvariantError::DecisionTakeUnavailable {
                        shot_id: shot.shot_id.clone(),
                        take_id: take_id.clone(),
                        kind,
                    });
                }
            }
            if let Some(approved) = &shot.approved_take_id
                && shot.selected_candidate_take_id.as_ref() != Some(approved)
            {
                return Err(StateInvariantError::ApprovedTakeNotSelected {
                    shot_id: shot.shot_id.clone(),
                    take_id: approved.clone(),
                });
            }
        }
        for approval in &self.pending_approvals {
            if approval.kind != ApprovalKind::CandidateSelection {
                continue;
            }
            let shot_id = approval.shot_id.as_ref().ok_or_else(|| {
                StateInvariantError::CandidateApprovalShotMissing(approval.approval_id.clone())
            })?;
            let shot = self.shots.get(shot_id).ok_or_else(|| {
                StateInvariantError::CandidateApprovalShotUnknown {
                    approval_id: approval.approval_id.clone(),
                    shot_id: shot_id.clone(),
                }
            })?;
            for take_id in &approval.take_ids {
                if !shot.take_ids.contains(take_id)
                    || shot.rejected_take_ids.contains(take_id)
                    || self.takes.get(take_id).is_none_or(|take| take.stale)
                {
                    return Err(StateInvariantError::CandidateApprovalTakeUnavailable {
                        approval_id: approval.approval_id.clone(),
                        take_id: take_id.clone(),
                    });
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContractStatus {
    PendingApproval,
    Active,
    Superseded,
    Rejected,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContractRecord {
    pub contract_id: String,
    pub relative_path: PathBuf,
    pub bundle_hash: String,
    pub status: ContractStatus,
    pub receipt: AuthoringReceipt,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthoringReceipt {
    pub receipt_id: String,
    pub contract_id: String,
    pub project_id: String,
    pub schema_version: String,
    pub bundle_hash: String,
    pub brief_hash: String,
    pub command_id: String,
    pub skill: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalKind {
    ScriptBundle,
    CandidateSelection,
    BudgetOverrun,
    FinalVisualReview,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Approval {
    pub approval_id: String,
    pub kind: ApprovalKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shot_id: Option<String>,
    #[serde(default)]
    pub take_ids: Vec<String>,
    pub blocking: bool,
    pub description: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShotStage {
    Pending,
    Queued,
    Generating,
    CandidatesReady,
    Selected,
    Approved,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShotRuntimeState {
    pub shot_id: String,
    pub title: String,
    pub stage: ShotStage,
    pub risk: Risk,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_job_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_candidate_take_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approved_take_id: Option<String>,
    #[serde(default)]
    pub take_ids: Vec<String>,
    #[serde(default)]
    pub rejected_take_ids: Vec<String>,
    #[serde(default)]
    pub fail_codes: Vec<String>,
    pub stale: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FailureRecord {
    pub code: String,
    pub subject: String,
    pub message: String,
    pub occurred_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BibleIndex {
    pub schema_version: String,
    pub characters: BTreeMap<String, BibleEntry>,
    pub locations: BTreeMap<String, BibleEntry>,
    pub style_source: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BibleEntry {
    pub source: PathBuf,
    #[serde(default)]
    pub references: Vec<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Queued,
    Active,
    Completed,
    Blocked,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttemptState {
    Prepared,
    Submitting,
    SubmissionUnknown,
    Submitted,
    Running,
    BackendSucceeded,
    BackendFailed,
    RetryWait,
    OutputValidated,
    OutputInvalid,
    Completed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JobJournal {
    pub schema_version: String,
    pub job_id: String,
    pub command_id: String,
    pub project_id: String,
    pub contract_id: String,
    pub shot_id: String,
    pub reserved_take_id: String,
    pub operation: Operation,
    pub resolved_prompt: String,
    pub seed: u64,
    pub profile: String,
    pub input_hash: String,
    pub adapter_fingerprint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_take_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub promotion_strategy: Option<PromotionStrategy>,
    pub state: JobState,
    #[serde(default)]
    pub attempts: Vec<AttemptJournal>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttemptJournal {
    pub request_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend_job_id: Option<String>,
    pub state: AttemptState,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_path: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TakeMetadata {
    pub take_id: String,
    pub shot_id: String,
    pub job_id: String,
    pub profile: String,
    pub status: String,
    pub media_path: PathBuf,
    pub input_hash: String,
    pub adapter_fingerprint: String,
    pub workflow_hash: String,
    pub model_fingerprint: String,
    pub seed: u64,
    pub elapsed_milliseconds: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_frame_path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_frame_path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handoff_candidate_path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_take_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub promotion_strategy: Option<PromotionStrategy>,
    #[serde(default)]
    pub hard_checks: Vec<String>,
    #[serde(default)]
    pub warnings: Vec<String>,
    pub stale: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromotionStrategy {
    Enhance,
    VideoReference,
    FrameReference,
    SeedReplay,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildRecord {
    pub build_id: String,
    pub kind: String,
    pub status: String,
    pub recipe: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_path: Option<PathBuf>,
    #[serde(default)]
    pub warnings: Vec<String>,
    pub stale: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueueState {
    pub schema_version: String,
    pub revision: u64,
    pub paused: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_command_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub running: Option<QueueEntry>,
    #[serde(default)]
    pub pending: Vec<QueueEntry>,
}

impl Default for QueueState {
    fn default() -> Self {
        Self {
            schema_version: PROJECT_SCHEMA_VERSION.to_owned(),
            revision: 1,
            paused: false,
            last_command_id: None,
            running: None,
            pending: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueueEntry {
    pub project_id: String,
    pub project_path: PathBuf,
    pub job_id: String,
    pub priority: String,
    pub resource: String,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum StateInvariantError {
    #[error("unsupported project schema `{0}`")]
    UnsupportedSchema(String),
    #[error("project revision must be greater than zero")]
    ZeroRevision,
    #[error("project revision overflow")]
    RevisionOverflow,
    #[error("active contract is missing from the contract index")]
    MissingActiveContract,
    #[error("project outcome does not match blocking approvals")]
    OutcomeApprovalMismatch,
    #[error("duplicate approval id `{0}`")]
    DuplicateApproval(String),
    #[error("shot map key `{key}` does not match shot id `{shot_id}`")]
    ShotKeyMismatch { key: String, shot_id: String },
    #[error("take map key `{key}` does not match take id `{take_id}`")]
    TakeKeyMismatch { key: String, take_id: String },
    #[error("shot `{shot_id}` lists take `{take_id}` more than once")]
    DuplicateShotTake { shot_id: String, take_id: String },
    #[error("shot `{shot_id}` lists missing take `{take_id}`")]
    ShotTakeMissing { shot_id: String, take_id: String },
    #[error("take `{take_id}` belongs to shot `{actual_shot_id}`, expected `{expected_shot_id}`")]
    TakeShotMismatch {
        take_id: String,
        expected_shot_id: String,
        actual_shot_id: String,
    },
    #[error("shot `{shot_id}` rejects unavailable take `{take_id}`")]
    RejectedTakeUnavailable { shot_id: String, take_id: String },
    #[error("shot `{shot_id}` has unavailable {kind} take `{take_id}`")]
    DecisionTakeUnavailable {
        shot_id: String,
        take_id: String,
        kind: &'static str,
    },
    #[error("shot `{shot_id}` approves take `{take_id}` without selecting it")]
    ApprovedTakeNotSelected { shot_id: String, take_id: String },
    #[error("candidate approval `{0}` has no shot id")]
    CandidateApprovalShotMissing(String),
    #[error("candidate approval `{approval_id}` refers to missing shot `{shot_id}`")]
    CandidateApprovalShotUnknown {
        approval_id: String,
        shot_id: String,
    },
    #[error("candidate approval `{approval_id}` lists unavailable take `{take_id}`")]
    CandidateApprovalTakeUnavailable {
        approval_id: String,
        take_id: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocking_approval_derives_needs_review() {
        let mut state = ProjectState::new("demo".to_owned(), "Demo".to_owned(), "1".to_owned());
        state.pending_approvals.push(Approval {
            approval_id: "APR-1".to_owned(),
            kind: ApprovalKind::ScriptBundle,
            subject_id: Some("CON-1".to_owned()),
            shot_id: None,
            take_ids: vec![],
            blocking: true,
            description: "Review script".to_owned(),
            created_at: "1".to_owned(),
        });

        state.bump_revision("2".to_owned()).unwrap();
        assert_eq!(state.project_outcome, ProjectOutcome::NeedsReview);
        assert_eq!(state.revision, 2);
    }

    #[test]
    fn active_contract_must_exist() {
        let mut state = ProjectState::new("demo".to_owned(), "Demo".to_owned(), "1".to_owned());
        state.active_contract_id = Some("missing".to_owned());

        assert_eq!(
            state.validate(),
            Err(StateInvariantError::MissingActiveContract)
        );
    }

    #[test]
    fn shot_take_references_must_resolve() {
        let mut state = ProjectState::new("demo".to_owned(), "Demo".to_owned(), "1".to_owned());
        state.shots.insert(
            "S01".to_owned(),
            ShotRuntimeState {
                shot_id: "S01".to_owned(),
                title: "Shot 1".to_owned(),
                stage: ShotStage::CandidatesReady,
                risk: Risk::Low,
                active_job_id: None,
                selected_candidate_take_id: None,
                approved_take_id: None,
                take_ids: vec!["TAKE-missing".to_owned()],
                rejected_take_ids: Vec::new(),
                fail_codes: Vec::new(),
                stale: false,
            },
        );

        assert!(matches!(
            state.validate(),
            Err(StateInvariantError::ShotTakeMissing { .. })
        ));
    }
}
