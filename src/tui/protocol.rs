use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub const IPC_PROTOCOL_VERSION: &str = "1.0";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClientRequest {
    pub protocol_version: String,
    pub command_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_revision: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    pub command: WorkerCommand,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum WorkerCommand {
    Health,
    Snapshot,
    Subscribe {
        project_revision: u64,
        queue_revision: u64,
    },
    CreateProject {
        project_id: String,
        title: String,
        brief: String,
    },
    ApplyScript {
        bundle_json: String,
    },
    ApproveScript,
    Approve {
        approval_id: String,
    },
    RetryShot {
        shot_id: String,
    },
    AuditionShot {
        shot_id: String,
    },
    RenderShot {
        shot_id: String,
    },
    SelectTake {
        shot_id: String,
        take_id: String,
    },
    ApproveTake {
        shot_id: String,
        take_id: String,
    },
    RejectTake {
        shot_id: String,
        take_id: String,
    },
    PreviewTake {
        take_id: String,
    },
    PauseQueue,
    ResumeQueue,
    CancelJob {
        job_id: String,
    },
    Build {
        kind: String,
        #[serde(default)]
        shot_ids: Vec<String>,
    },
    OpenBuild {
        build_id: String,
    },
    RetryProbe {
        probe_id: String,
    },
    OpenLogs,
}

impl WorkerCommand {
    #[must_use]
    pub fn is_mutating(&self) -> bool {
        !matches!(
            self,
            Self::Health
                | Self::Snapshot
                | Self::Subscribe { .. }
                | Self::PreviewTake { .. }
                | Self::OpenBuild { .. }
                | Self::RetryProbe { .. }
                | Self::OpenLogs
        )
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerReply {
    pub protocol_version: String,
    pub command_id: String,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<AppSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<WorkerError>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerError {
    pub code: String,
    pub message: String,
    #[serde(default)]
    pub retryable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_revision: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RevisionEvent {
    pub protocol_version: String,
    pub project_id: String,
    pub project_revision: u64,
    pub queue_revision: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppSnapshot {
    pub schema_version: String,
    pub revision: u64,
    pub refreshed_at: String,
    pub project: ProjectSummary,
    #[serde(default)]
    pub gpu: GpuSummary,
    #[serde(default)]
    pub budget: BudgetSummary,
    #[serde(default)]
    pub pending_approvals: Vec<ApprovalSummary>,
    #[serde(default)]
    pub recent_failures: Vec<FailureSummary>,
    #[serde(default)]
    pub shots: Vec<ShotSummary>,
    #[serde(default)]
    pub takes: Vec<TakeSummary>,
    #[serde(default)]
    pub queue: QueueSummary,
    #[serde(default)]
    pub builds: Vec<BuildSummary>,
    #[serde(default)]
    pub diagnostics: Vec<DiagnosticSummary>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectSummary {
    pub id: String,
    pub title: String,
    pub stage: String,
    pub outcome: String,
    pub work_mode: String,
    pub quality_target: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GpuSummary {
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shot_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eta_seconds: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BudgetSummary {
    pub elapsed_seconds: u64,
    pub estimated_remaining_seconds: u64,
    pub disk_free_bytes: u64,
    pub disk_required_bytes: u64,
    pub audition_takes_used: u32,
    pub audition_takes_limit: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovalSummary {
    pub approval_id: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shot_id: Option<String>,
    #[serde(default)]
    pub take_ids: Vec<String>,
    pub blocking: bool,
    pub description: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FailureSummary {
    pub code: String,
    pub subject: String,
    pub message: String,
    pub occurred_at: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShotSummary {
    pub shot_id: String,
    pub title: String,
    pub stage: String,
    pub risk: String,
    pub candidate_count: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_take_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approved_take_id: Option<String>,
    #[serde(default)]
    pub fail_codes: Vec<String>,
    pub stale: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TakeSummary {
    pub take_id: String,
    pub shot_id: String,
    pub profile: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
    #[serde(default)]
    pub hard_checks: Vec<String>,
    #[serde(default)]
    pub warnings: Vec<String>,
    pub selected: bool,
    pub approved: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueueSummary {
    #[serde(default)]
    pub revision: u64,
    pub paused: bool,
    #[serde(default)]
    pub jobs: Vec<QueueJobSummary>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueueJobSummary {
    pub job_id: String,
    pub subject: String,
    pub state: String,
    pub priority: String,
    pub resource: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eta_seconds: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildSummary {
    pub build_id: String,
    pub kind: String,
    pub status: String,
    pub recipe: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub command_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_path: Option<PathBuf>,
    #[serde(default)]
    pub warnings: Vec<String>,
    pub stale: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticSummary {
    pub probe_id: String,
    pub component: String,
    pub status: String,
    pub summary: String,
    #[serde(default)]
    pub capabilities: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::{QueueSummary, WorkerCommand};

    #[test]
    fn queue_summary_without_revision_defaults_to_zero() {
        let summary: QueueSummary = serde_json::from_str(r#"{"paused":false,"jobs":[]}"#).unwrap();

        assert_eq!(summary.revision, 0);
    }

    #[test]
    fn legacy_full_build_command_defaults_to_all_shots() {
        let command: WorkerCommand =
            serde_json::from_str(r#"{"type":"build","payload":{"kind":"draft"}}"#).unwrap();

        assert_eq!(
            command,
            WorkerCommand::Build {
                kind: "draft".to_owned(),
                shot_ids: Vec::new(),
            }
        );
    }
}
