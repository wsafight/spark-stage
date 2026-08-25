use super::*;
use crate::domain::{Risk, ShotRuntimeState, ShotStage, TakeMetadata};

const REJECTED: &str = "TAKE-00000000000000000000000000";
const SELECTED: &str = "TAKE-00000000000000000000000001";

fn take(take_id: &str) -> TakeMetadata {
    TakeMetadata {
        take_id: take_id.to_owned(),
        shot_id: "S01".to_owned(),
        job_id: "JOB-00000000000000000000000000".to_owned(),
        profile: "audition".to_owned(),
        status: "candidate".to_owned(),
        media_path: PathBuf::from(format!("raw/S01/{take_id}.mp4")),
        input_hash: "input".to_owned(),
        adapter_fingerprint: "adapter".to_owned(),
        workflow_hash: "workflow".to_owned(),
        model_fingerprint: "model".to_owned(),
        seed: 1,
        elapsed_milliseconds: 10,
        first_frame_path: None,
        last_frame_path: None,
        handoff_candidate_path: None,
        parent_take_id: None,
        promotion_strategy: None,
        hard_checks: Vec::new(),
        warnings: Vec::new(),
        stale: false,
    }
}

fn store() -> (tempfile::TempDir, ProjectStore) {
    let directory = tempfile::tempdir().unwrap();
    let store =
        ProjectStore::create(directory.path(), "demo", "Demo", "Brief", "create", "100").unwrap();
    let rejected = take(REJECTED);
    let selected = take(SELECTED);
    for (take, bytes) in [
        (&rejected, b"rejected".as_slice()),
        (&selected, b"selected"),
    ] {
        let path = store.root().join(&take.media_path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
        store.save_take_metadata(take).unwrap();
    }
    let mut state = store.read_state().unwrap();
    state.takes.insert(REJECTED.to_owned(), rejected);
    state.takes.insert(SELECTED.to_owned(), selected);
    state.shots.insert(
        "S01".to_owned(),
        ShotRuntimeState {
            shot_id: "S01".to_owned(),
            title: "Shot 1".to_owned(),
            stage: ShotStage::Selected,
            risk: Risk::Low,
            active_job_id: None,
            audition_target_takes: None,
            selected_candidate_take_id: Some(SELECTED.to_owned()),
            approved_take_id: None,
            take_ids: vec![REJECTED.to_owned(), SELECTED.to_owned()],
            rejected_take_ids: vec![REJECTED.to_owned()],
            fail_codes: Vec::new(),
            stale: false,
        },
    );
    state.bump_revision("101".to_owned()).unwrap();
    store.save_state(&state, 1).unwrap();
    (directory, store)
}

#[test]
fn cleanup_plan_apply_and_restore_round_trip() {
    let (_directory, store) = store();
    let report = store.storage_report().unwrap();
    assert_eq!(report.reclaimable_files, 1);
    assert_eq!(report.reclaimable_bytes, 8);

    let (planned_state, plan) = store.create_cleanup_plan(2, "plan", "102").unwrap();
    assert_eq!(plan.items.len(), 1);
    assert_eq!(plan.items[0].subject_id, REJECTED);
    let rejected_path = store.root().join(&plan.items[0].path);
    let selected_path = store
        .root()
        .join(&store.read_state().unwrap().takes[SELECTED].media_path);

    let (applied_state, applied) = store
        .apply_cleanup_plan(&plan.plan_id, planned_state.revision, "apply", "103")
        .unwrap();
    assert_eq!(applied.status, CleanupPlanStatus::Applied);
    assert!(!rejected_path.exists());
    assert_eq!(fs::read(&selected_path).unwrap(), b"selected");

    let (_restored_state, restored) = store
        .restore_cleanup_plan(&plan.plan_id, applied_state.revision, "restore", "104")
        .unwrap();
    assert_eq!(restored.status, CleanupPlanStatus::Restored);
    assert_eq!(fs::read(rejected_path).unwrap(), b"rejected");
}

#[test]
fn interrupted_apply_and_restore_resume_from_file_locations() {
    let (_directory, store) = store();
    let first_frame = PathBuf::from(format!("raw/S01/{REJECTED}-first.png"));
    let first_frame_path = store.root().join(&first_frame);
    fs::write(&first_frame_path, b"frame").unwrap();
    let mut state = store.read_state().unwrap();
    state.takes.get_mut(REJECTED).unwrap().first_frame_path = Some(first_frame);
    state.bump_revision("102".to_owned()).unwrap();
    store.save_state(&state, 2).unwrap();

    let (planned_state, mut plan) = store.create_cleanup_plan(3, "plan", "103").unwrap();
    assert_eq!(plan.items.len(), 2);
    store
        .prepare_decision("cleanup_applied", &plan.plan_id, "apply-crash", "104")
        .unwrap();
    plan.status = CleanupPlanStatus::Applying;
    plan.active_operation = Some(CleanupOperation {
        command_id: "apply-crash".to_owned(),
        source_revision: planned_state.revision,
        started_at: "104".to_owned(),
    });
    let plan_path = cleanup_plan_path(store.root(), &plan.plan_id).unwrap();
    write_json_atomic(&plan_path, &plan).unwrap();
    let first_original = store.root().join(&plan.items[0].path);
    let first_trashed =
        cleanup_trash_path(store.root(), &plan.plan_id, &plan.items[0].path).unwrap();
    fs::create_dir_all(first_trashed.parent().unwrap()).unwrap();
    fs::rename(&first_original, &first_trashed).unwrap();

    assert_eq!(store.recover_cleanup_plans().unwrap(), 1);
    let applied: CleanupPlan = read_json(&plan_path).unwrap();
    assert_eq!(applied.status, CleanupPlanStatus::Applied);
    assert!(applied.active_operation.is_none());
    for item in &applied.items {
        assert!(!store.root().join(&item.path).exists());
        assert!(
            cleanup_trash_path(store.root(), &applied.plan_id, &item.path)
                .unwrap()
                .is_file()
        );
    }
    let applied_state = store.read_state().unwrap();
    assert_eq!(
        applied_state.last_command_id.as_deref(),
        Some("apply-crash")
    );
    assert_eq!(
        store
            .decision_history(10)
            .unwrap()
            .iter()
            .filter(|decision| decision.command_id == "apply-crash")
            .count(),
        1
    );

    store
        .prepare_decision("cleanup_restored", &applied.plan_id, "restore-crash", "105")
        .unwrap();
    let mut restoring = applied;
    restoring.status = CleanupPlanStatus::Restoring;
    restoring.active_operation = Some(CleanupOperation {
        command_id: "restore-crash".to_owned(),
        source_revision: applied_state.revision,
        started_at: "105".to_owned(),
    });
    write_json_atomic(&plan_path, &restoring).unwrap();
    let first_original = store.root().join(&restoring.items[0].path);
    let first_trashed =
        cleanup_trash_path(store.root(), &restoring.plan_id, &restoring.items[0].path).unwrap();
    fs::create_dir_all(first_original.parent().unwrap()).unwrap();
    fs::rename(&first_trashed, &first_original).unwrap();

    assert_eq!(store.recover_cleanup_plans().unwrap(), 1);
    let restored: CleanupPlan = read_json(&plan_path).unwrap();
    assert_eq!(restored.status, CleanupPlanStatus::Restored);
    for item in &restored.items {
        assert!(store.root().join(&item.path).is_file());
        assert!(
            !cleanup_trash_path(store.root(), &restored.plan_id, &item.path)
                .unwrap()
                .exists()
        );
    }
    assert_eq!(
        store
            .decision_history(10)
            .unwrap()
            .iter()
            .filter(|decision| decision.command_id == "restore-crash")
            .count(),
        1
    );
}

#[test]
fn changed_take_decision_makes_cleanup_plan_stale() {
    let (_directory, store) = store();
    let (planned_state, plan) = store.create_cleanup_plan(2, "plan", "102").unwrap();
    let mut state = planned_state;
    let shot = state.shots.get_mut("S01").unwrap();
    shot.rejected_take_ids.clear();
    shot.selected_candidate_take_id = Some(REJECTED.to_owned());
    state.bump_revision("103".to_owned()).unwrap();
    store.save_state(&state, 3).unwrap();

    let error = store
        .apply_cleanup_plan(&plan.plan_id, 4, "apply", "104")
        .unwrap_err();

    assert!(matches!(error, StoreError::CleanupPlanStale(_)));
    assert!(store.root().join(&plan.items[0].path).is_file());
}

#[test]
fn tampered_cleanup_path_is_rejected() {
    let (_directory, store) = store();
    let (_state, mut plan) = store.create_cleanup_plan(2, "plan", "102").unwrap();
    plan.items[0].path = PathBuf::from("../outside.mp4");
    write_json_atomic(
        &cleanup_plan_path(store.root(), &plan.plan_id).unwrap(),
        &plan,
    )
    .unwrap();

    let error = store
        .apply_cleanup_plan(&plan.plan_id, 3, "apply", "103")
        .unwrap_err();

    assert!(matches!(error, StoreError::InvalidCleanupPlan(_)));
}

#[test]
fn restore_never_overwrites_a_new_file() {
    let (_directory, store) = store();
    let (planned_state, plan) = store.create_cleanup_plan(2, "plan", "102").unwrap();
    let (applied_state, _) = store
        .apply_cleanup_plan(&plan.plan_id, planned_state.revision, "apply", "103")
        .unwrap();
    let original = store.root().join(&plan.items[0].path);
    fs::create_dir_all(original.parent().unwrap()).unwrap();
    fs::write(&original, b"new-content").unwrap();

    let error = store
        .restore_cleanup_plan(&plan.plan_id, applied_state.revision, "restore", "104")
        .unwrap_err();

    assert!(matches!(error, StoreError::CleanupPathConflict(_)));
    assert_eq!(fs::read(original).unwrap(), b"new-content");
    assert!(
        cleanup_trash_path(store.root(), &plan.plan_id, &plan.items[0].path)
            .unwrap()
            .is_file()
    );
}

#[cfg(unix)]
#[test]
fn cleanup_rejects_project_paths_with_symlink_ancestors() {
    use std::os::unix::fs::symlink;

    let (_directory, store) = store();
    let outside = tempfile::tempdir().unwrap();
    let outside_file = outside.path().join(format!("{REJECTED}.mp4"));
    fs::write(&outside_file, b"outside").unwrap();
    fs::remove_dir_all(store.root().join("raw/S01")).unwrap();
    symlink(outside.path(), store.root().join("raw/S01")).unwrap();

    let error = store.storage_report().unwrap_err();

    assert!(matches!(error, StoreError::InvalidCleanupPlan(_)));
    assert_eq!(fs::read(outside_file).unwrap(), b"outside");
}

#[cfg(unix)]
#[test]
fn cleanup_apply_rejects_symlinked_trash_directory() {
    use std::os::unix::fs::symlink;

    let (_directory, store) = store();
    let (state, plan) = store.create_cleanup_plan(2, "plan", "102").unwrap();
    let outside = tempfile::tempdir().unwrap();
    symlink(
        outside.path(),
        store.root().join("trash").join(&plan.plan_id),
    )
    .unwrap();

    let error = store
        .apply_cleanup_plan(&plan.plan_id, state.revision, "apply", "103")
        .unwrap_err();

    assert!(matches!(error, StoreError::InvalidCleanupPlan(_)));
    assert!(store.root().join(&plan.items[0].path).is_file());
    assert!(fs::read_dir(outside.path()).unwrap().next().is_none());
}
