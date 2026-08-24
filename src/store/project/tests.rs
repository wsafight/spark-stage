use super::*;
use crate::validation::validate_json;

const BUNDLE: &str = include_str!("../../../skills/screenwriter/examples/valid-short-drama.json");

fn take(take_id: &str, shot_id: &str, stale: bool) -> TakeMetadata {
    TakeMetadata {
        take_id: take_id.to_owned(),
        shot_id: shot_id.to_owned(),
        job_id: "JOB-00000000000000000000000000".to_owned(),
        profile: "audition".to_owned(),
        status: "candidate".to_owned(),
        media_path: PathBuf::from(format!("raw/{shot_id}/{take_id}.mp4")),
        input_hash: "input".to_owned(),
        adapter_fingerprint: "adapter".to_owned(),
        workflow_hash: "workflow".to_owned(),
        model_fingerprint: "model".to_owned(),
        seed: 7,
        elapsed_milliseconds: 100,
        first_frame_path: None,
        last_frame_path: None,
        handoff_candidate_path: None,
        parent_take_id: None,
        promotion_strategy: None,
        hard_checks: Vec::new(),
        warnings: Vec::new(),
        stale,
    }
}

fn decision_store() -> (tempfile::TempDir, ProjectStore, String) {
    let directory = tempfile::tempdir().unwrap();
    let store = ProjectStore::create(
        directory.path(),
        "demo",
        "Demo",
        "Brief",
        "01ARZ3NDEKTSV4RRFFQ69G5FAV",
        "100",
    )
    .unwrap();
    let take_id = "TAKE-00000000000000000000000000".to_owned();
    let mut state = store.read_state().unwrap();
    let mut shot = initial_shot_state(
        "S01".to_owned(),
        "Shot 1".to_owned(),
        crate::domain::Risk::Low,
    );
    shot.stage = ShotStage::CandidatesReady;
    shot.take_ids.push(take_id.clone());
    state.shots.insert("S01".to_owned(), shot);
    state.shots.insert(
        "S02".to_owned(),
        initial_shot_state(
            "S02".to_owned(),
            "Shot 2".to_owned(),
            crate::domain::Risk::Low,
        ),
    );
    state
        .takes
        .insert(take_id.clone(), take(&take_id, "S01", false));
    state.bump_revision("101".to_owned()).unwrap();
    store.save_state(&state, 1).unwrap();
    (directory, store, take_id)
}

#[test]
fn project_creation_is_complete_and_readable() {
    let directory = tempfile::tempdir().unwrap();
    let store = ProjectStore::create(
        directory.path(),
        "rain-apartment",
        "Rain Apartment",
        "A meeting in the rain.",
        "01ARZ3NDEKTSV4RRFFQ69G5FAV",
        "100",
    )
    .unwrap();

    assert_eq!(store.read_state().unwrap().revision, 1);
    assert!(store.root().join("script/brief.md").is_file());
    assert!(store.root().join("contracts").is_dir());
}

#[test]
fn bundle_requires_approval_before_becoming_active() {
    let directory = tempfile::tempdir().unwrap();
    let bundle = validate_json(BUNDLE).bundle.unwrap();
    let store = ProjectStore::create(
        directory.path(),
        &bundle.project.id,
        &bundle.project.title,
        "A brief.",
        "01ARZ3NDEKTSV4RRFFQ69G5FAW",
        "100",
    )
    .unwrap();

    let (pending, approval) = store
        .apply_bundle(&bundle, 1, "01ARZ3NDEKTSV4RRFFQ69G5FAX", "101")
        .unwrap();
    assert_eq!(pending.revision, 2);
    assert!(pending.active_contract_id.is_none());
    assert!(pending.project_outcome == crate::domain::ProjectOutcome::NeedsReview);

    let active = store
        .approve_script(
            Some(&approval.approval_id),
            2,
            "01ARZ3NDEKTSV4RRFFQ69G5FAY",
            "102",
        )
        .unwrap();
    assert_eq!(active.revision, 3);
    assert!(active.active_contract_id.is_some());
    assert_eq!(active.shots.len(), bundle.shots.len());
    assert!(store.root().join("active-contract.json").is_file());
    assert_eq!(store.read_active_bundle().unwrap(), Some(bundle));
}

#[test]
fn stale_revision_cannot_overwrite_state() {
    let directory = tempfile::tempdir().unwrap();
    let store = ProjectStore::create(
        directory.path(),
        "demo",
        "Demo",
        "Brief",
        "01ARZ3NDEKTSV4RRFFQ69G5FAZ",
        "100",
    )
    .unwrap();
    let mut state = store.read_state().unwrap();
    state.bump_revision("101".to_owned()).unwrap();
    store.save_state(&state, 1).unwrap();

    let error = store.save_state(&state, 1).unwrap_err();
    assert!(matches!(
        error,
        StoreError::RevisionConflict {
            expected: 1,
            actual: 2
        }
    ));
}

#[test]
fn take_approval_requires_selection_and_blocks_later_rejection() {
    let (_directory, store, take_id) = decision_store();

    let error = store
        .approve_take("S01", &take_id, 2, "approve-too-early", "102")
        .unwrap_err();
    assert!(matches!(error, StoreError::TakeNotSelected(_)));

    let selected = store
        .select_take("S01", &take_id, 2, "select", "103")
        .unwrap();
    assert_eq!(selected.shots["S01"].stage, ShotStage::Selected);
    let approved = store
        .approve_take("S01", &take_id, 3, "approve", "104")
        .unwrap();
    assert_eq!(approved.shots["S01"].stage, ShotStage::Approved);

    let error = store
        .reject_take("S01", &take_id, 4, "reject", "105")
        .unwrap_err();
    assert!(matches!(error, StoreError::ShotAlreadyApproved(_)));
}

#[test]
fn rejecting_only_candidate_returns_shot_to_pending() {
    let (_directory, store, take_id) = decision_store();

    let state = store
        .reject_take("S01", &take_id, 2, "reject", "102")
        .unwrap();

    assert_eq!(state.shots["S01"].stage, ShotStage::Pending);
    assert_eq!(state.shots["S01"].rejected_take_ids, vec![take_id]);
}

#[test]
fn take_decisions_reject_stale_and_cross_shot_targets() {
    let (_directory, store, take_id) = decision_store();
    let mut state = store.read_state().unwrap();
    state.takes.get_mut(&take_id).unwrap().stale = true;
    state.bump_revision("102".to_owned()).unwrap();
    store.save_state(&state, 2).unwrap();

    let stale = store
        .select_take("S01", &take_id, 3, "select-stale", "103")
        .unwrap_err();
    assert!(matches!(stale, StoreError::TakeStale(_)));

    let mismatch = store
        .select_take("S02", &take_id, 3, "select-mismatch", "104")
        .unwrap_err();
    assert!(matches!(mismatch, StoreError::TakeShotMismatch { .. }));
}

#[test]
fn replacement_bundle_cannot_be_approved_while_a_shot_job_is_active() {
    let directory = tempfile::tempdir().unwrap();
    let mut bundle = validate_json(BUNDLE).bundle.unwrap();
    let store = ProjectStore::create(
        directory.path(),
        &bundle.project.id,
        &bundle.project.title,
        "A brief.",
        "create",
        "100",
    )
    .unwrap();
    let (_, initial_approval) = store
        .apply_bundle(&bundle, 1, "apply-initial", "101")
        .unwrap();
    let active = store
        .approve_script(
            Some(&initial_approval.approval_id),
            2,
            "approve-initial",
            "102",
        )
        .unwrap();
    let contract_id = active.active_contract_id.unwrap();
    let shot = &bundle.shots[0];
    let job = JobJournal {
        schema_version: crate::domain::PROJECT_SCHEMA_VERSION.to_owned(),
        job_id: "JOB-00000000000000000000000001".to_owned(),
        command_id: "enqueue".to_owned(),
        project_id: bundle.project.id.clone(),
        contract_id,
        shot_id: shot.id.clone(),
        reserved_take_id: "TAKE-00000000000000000000000001".to_owned(),
        operation: shot.operation,
        resolved_prompt: shot.prompt.clone(),
        seed: 1,
        profile: shot.generation_plan.audition_profile.clone(),
        input_hash: "input".to_owned(),
        adapter_fingerprint: "adapter".to_owned(),
        parent_take_id: None,
        promotion_strategy: None,
        state: JobState::Queued,
        attempts: Vec::new(),
    };
    store.enqueue_job(&job, 3, "enqueue", "103").unwrap();
    bundle.project.title = "Replacement".to_owned();
    let (_, replacement_approval) = store
        .apply_bundle(&bundle, 4, "apply-replacement", "104")
        .unwrap();

    let error = store
        .approve_script(
            Some(&replacement_approval.approval_id),
            5,
            "approve-replacement",
            "105",
        )
        .unwrap_err();

    assert!(matches!(error, StoreError::ShotBusy { .. }));
    assert_eq!(
        store.read_state().unwrap().active_contract_id,
        Some(job.contract_id)
    );
}
