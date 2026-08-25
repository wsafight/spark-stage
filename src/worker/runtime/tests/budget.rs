use super::*;

#[test]
fn snapshot_labels_unmeasured_budget_estimates() {
    let directory = tempfile::tempdir().unwrap();
    let paths = AppPaths::resolve(Some(directory.path().join("data")), None);
    let mut runtime = WorkerRuntime::open(paths).unwrap();
    approved_project(&mut runtime);

    let reply = runtime.handle(request(
        Some("rain-apartment"),
        None,
        WorkerCommand::Snapshot,
    ));

    let budget = reply.snapshot.unwrap().budget;
    assert_eq!(budget.estimate_source, "unmeasured_default_v1");
    assert!(budget.estimated_total_seconds > 0);
    assert_eq!(budget.wall_clock_limit_seconds, 4 * 60 * 60);
    assert_eq!(budget.max_audition_takes_per_shot, 3);
    assert_eq!(budget.max_final_takes_per_shot, 2);
    assert!(budget.disk_free_bytes > budget.disk_required_bytes);
}

#[test]
fn take_overrun_requires_approval_and_retry_uses_the_grant() {
    let directory = tempfile::tempdir().unwrap();
    let paths = AppPaths::resolve(Some(directory.path().join("data")), None);
    let adapter = verified_adapter(directory.path());
    let mut runtime = WorkerRuntime::open_with_adapter(paths.clone(), Some(adapter)).unwrap();
    approved_project(&mut runtime);
    seed_candidate(
        &paths,
        false,
        ShotStage::CandidatesReady,
        PathBuf::from("raw/S01/existing.mp4"),
    );
    let store = ProjectStore::open(&paths.projects_dir, "rain-apartment").unwrap();
    let mut state = store.read_state().unwrap();
    state.budget.contract.max_audition_takes_per_shot = 1;
    state.bump_revision("budget".to_owned()).unwrap();
    store.save_state(&state, 4).unwrap();

    let blocked = runtime.handle(request(
        Some("rain-apartment"),
        Some(5),
        WorkerCommand::AuditionShot {
            shot_id: "S01".to_owned(),
        },
    ));

    assert!(blocked.ok, "{blocked:?}");
    assert_eq!(blocked.revision, Some(6));
    let snapshot = blocked.snapshot.unwrap();
    let approval = snapshot
        .pending_approvals
        .iter()
        .find(|approval| approval.kind == "budget_overrun")
        .unwrap();
    let approval_id = approval.approval_id.clone();
    assert!(approval.description.contains("take 2"));
    assert!(runtime.queue.pending.is_empty());

    let approved = runtime.handle(request(
        Some("rain-apartment"),
        Some(6),
        WorkerCommand::Approve { approval_id },
    ));
    assert!(approved.ok, "{approved:?}");
    assert_eq!(approved.revision, Some(7));

    let retried = runtime.handle(request(
        Some("rain-apartment"),
        Some(7),
        WorkerCommand::AuditionShot {
            shot_id: "S01".to_owned(),
        },
    ));
    assert!(retried.ok, "{retried:?}");
    assert_eq!(retried.revision, Some(8));
    assert_eq!(runtime.queue.pending.len(), 1);
}

#[test]
fn minimum_disk_floor_cannot_be_bypassed_by_approval() {
    let directory = tempfile::tempdir().unwrap();
    let paths = AppPaths::resolve(Some(directory.path().join("data")), None);
    let adapter = verified_adapter(directory.path());
    let mut runtime = WorkerRuntime::open_with_adapter(paths.clone(), Some(adapter)).unwrap();
    approved_project(&mut runtime);
    let store = ProjectStore::open(&paths.projects_dir, "rain-apartment").unwrap();
    let mut state = store.read_state().unwrap();
    state.budget.contract.minimum_disk_free_bytes = u64::MAX;
    state.bump_revision("budget".to_owned()).unwrap();
    store.save_state(&state, 3).unwrap();

    let reply = runtime.handle(request(
        Some("rain-apartment"),
        Some(4),
        WorkerCommand::AuditionShot {
            shot_id: "S01".to_owned(),
        },
    ));

    assert!(!reply.ok);
    assert_eq!(reply.error.unwrap().code, "DISK_BUDGET_EXCEEDED");
    let state = store.read_state().unwrap();
    assert_eq!(state.revision, 4);
    assert!(
        state
            .pending_approvals
            .iter()
            .all(|approval| approval.kind != ApprovalKind::BudgetOverrun)
    );
}

#[test]
fn budget_contract_update_is_versioned_and_visible_in_snapshot() {
    let directory = tempfile::tempdir().unwrap();
    let paths = AppPaths::resolve(Some(directory.path().join("data")), None);
    let mut runtime = WorkerRuntime::open(paths.clone()).unwrap();
    approved_project(&mut runtime);
    let contract = crate::domain::BudgetContract {
        contract_revision: 99,
        wall_clock_limit_seconds: 7_200,
        max_audition_takes_per_shot: 2,
        estimate: crate::domain::BudgetEstimateProfile {
            source: "local_manual_baseline".to_owned(),
            ..crate::domain::BudgetEstimateProfile::default()
        },
        ..crate::domain::BudgetContract::default()
    };

    let reply = runtime.handle(request(
        Some("rain-apartment"),
        Some(3),
        WorkerCommand::UpdateBudget { contract },
    ));

    assert!(reply.ok, "{reply:?}");
    let budget = reply.snapshot.unwrap().budget;
    assert_eq!(budget.wall_clock_limit_seconds, 7_200);
    assert_eq!(budget.max_audition_takes_per_shot, 2);
    assert_eq!(budget.estimate_source, "local_manual_baseline");
    let store = ProjectStore::open(&paths.projects_dir, "rain-apartment").unwrap();
    assert_eq!(
        store
            .read_state()
            .unwrap()
            .budget
            .contract
            .contract_revision,
        2
    );
}
