use super::*;
use crate::store::CleanupPlanStatus;

#[test]
fn storage_commands_plan_apply_and_restore_empty_project() {
    let (_directory, mut runtime) = runtime();
    assert!(
        runtime
            .handle(request(
                None,
                None,
                WorkerCommand::CreateProject {
                    project_id: "demo".to_owned(),
                    title: "Demo".to_owned(),
                    brief: "Brief".to_owned(),
                },
            ))
            .ok
    );

    let status = runtime.handle(request(Some("demo"), None, WorkerCommand::StorageStatus));
    assert!(status.ok, "{status:?}");
    assert_eq!(status.revision, Some(1));
    let Some(WorkerPayload::StorageReport(report)) = status.payload else {
        panic!("expected storage report");
    };
    assert_eq!(report.project_id, "demo");
    assert_eq!(report.reclaimable_files, 0);

    let planned = runtime.handle(request(
        Some("demo"),
        Some(1),
        WorkerCommand::CreateCleanupPlan,
    ));
    assert!(planned.ok, "{planned:?}");
    let Some(WorkerPayload::CleanupPlan(plan)) = planned.payload else {
        panic!("expected cleanup plan");
    };
    assert!(plan.items.is_empty());

    let applied = runtime.handle(request(
        Some("demo"),
        planned.revision,
        WorkerCommand::ApplyCleanupPlan {
            plan_id: plan.plan_id.clone(),
        },
    ));
    assert!(applied.ok, "{applied:?}");

    let restored = runtime.handle(request(
        Some("demo"),
        applied.revision,
        WorkerCommand::RestoreCleanupPlan {
            plan_id: plan.plan_id,
        },
    ));
    assert!(restored.ok, "{restored:?}");
    let Some(WorkerPayload::CleanupPlan(restored_plan)) = restored.payload else {
        panic!("expected restored cleanup plan");
    };
    assert_eq!(restored_plan.status, CleanupPlanStatus::Restored);
}

#[test]
fn storage_mutation_requires_current_revision() {
    let (_directory, mut runtime) = runtime();
    assert!(
        runtime
            .handle(request(
                None,
                None,
                WorkerCommand::CreateProject {
                    project_id: "demo".to_owned(),
                    title: "Demo".to_owned(),
                    brief: "Brief".to_owned(),
                },
            ))
            .ok
    );

    let missing = runtime.handle(request(
        Some("demo"),
        None,
        WorkerCommand::CreateCleanupPlan,
    ));
    assert!(!missing.ok);
    assert_eq!(missing.error.unwrap().code, "EXPECTED_REVISION_REQUIRED");

    let stale = runtime.handle(request(
        Some("demo"),
        Some(99),
        WorkerCommand::CreateCleanupPlan,
    ));
    assert!(!stale.ok);
    assert_eq!(stale.error.unwrap().code, "REVISION_CONFLICT");
    assert_eq!(stale.revision, Some(1));
}

#[test]
fn storage_status_rejects_revision_and_invalid_plan_id() {
    let (_directory, mut runtime) = runtime();
    assert!(
        runtime
            .handle(request(
                None,
                None,
                WorkerCommand::CreateProject {
                    project_id: "demo".to_owned(),
                    title: "Demo".to_owned(),
                    brief: "Brief".to_owned(),
                },
            ))
            .ok
    );

    let status = runtime.handle(request(Some("demo"), Some(1), WorkerCommand::StorageStatus));
    assert!(!status.ok);
    assert_eq!(status.error.unwrap().code, "INVALID_ARGUMENT");

    let invalid = runtime.handle(request(
        Some("demo"),
        Some(1),
        WorkerCommand::ApplyCleanupPlan {
            plan_id: "../state.json".to_owned(),
        },
    ));
    assert!(!invalid.ok);
    assert_eq!(invalid.error.unwrap().code, "INVALID_CLEANUP_PLAN");
}
