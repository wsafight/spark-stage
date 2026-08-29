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
    let saved = fs::read(store.state_path()).unwrap();

    let error = store.save_state(&state, 1).unwrap_err();
    assert!(matches!(
        error,
        StoreError::RevisionConflict {
            expected: 1,
            actual: 2
        }
    ));
    assert_eq!(fs::read(store.state_path()).unwrap(), saved);
    assert_eq!(store.read_state().unwrap().revision, 2);
}

#[test]
fn corrupt_state_is_reported_without_being_overwritten() {
    let directory = tempfile::tempdir().unwrap();
    let store =
        ProjectStore::create(directory.path(), "demo", "Demo", "Brief", "create", "100").unwrap();
    let mut replacement = store.read_state().unwrap();
    replacement.bump_revision("101".to_owned()).unwrap();
    let corrupt = br#"{"schema_version":"1.0","revision":#"#;
    fs::write(store.state_path(), corrupt).unwrap();

    let read_error = store.read_state().unwrap_err();
    assert!(matches!(read_error, StoreError::Decode { .. }));
    let save_error = store.save_state(&replacement, 1).unwrap_err();
    assert!(matches!(save_error, StoreError::Decode { .. }));
    assert_eq!(fs::read(store.state_path()).unwrap(), corrupt);
}

#[test]
fn unsupported_state_schema_is_rejected_without_being_overwritten() {
    let directory = tempfile::tempdir().unwrap();
    let store =
        ProjectStore::create(directory.path(), "demo", "Demo", "Brief", "create", "100").unwrap();
    let mut replacement = store.read_state().unwrap();
    replacement.bump_revision("101".to_owned()).unwrap();
    let mut encoded: serde_json::Value =
        serde_json::from_slice(&fs::read(store.state_path()).unwrap()).unwrap();
    encoded["schema_version"] = serde_json::Value::String("999.0".to_owned());
    let encoded = serde_json::to_vec_pretty(&encoded).unwrap();
    fs::write(store.state_path(), &encoded).unwrap();

    let read_error = store.read_state().unwrap_err();
    assert!(matches!(
        read_error,
        StoreError::Invariant(crate::domain::StateInvariantError::UnsupportedSchema(ref schema))
            if schema == "999.0"
    ));
    let save_error = store.save_state(&replacement, 1).unwrap_err();
    assert!(matches!(
        save_error,
        StoreError::Invariant(crate::domain::StateInvariantError::UnsupportedSchema(ref schema))
            if schema == "999.0"
    ));
    assert_eq!(fs::read(store.state_path()).unwrap(), encoded);
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
fn changing_selected_take_marks_affected_build_stale() {
    let (_directory, store, first_take_id) = decision_store();
    let selected = store
        .select_take("S01", &first_take_id, 2, "select-first", "102")
        .unwrap();
    let second_take_id = "TAKE-00000000000000000000000001".to_owned();
    let mut state = selected;
    state
        .takes
        .insert(second_take_id.clone(), take(&second_take_id, "S01", false));
    state
        .shots
        .get_mut("S01")
        .unwrap()
        .take_ids
        .push(second_take_id.clone());
    let build_id = "BLD-selection";
    let recipe_path = PathBuf::from("builds/BLD-selection/recipe.json");
    let first = &state.takes[&first_take_id];
    write_json_atomic(
        &store.root().join(&recipe_path),
        &crate::build::BuildRecipe {
            schema_version: crate::build::BUILD_RECIPE_SCHEMA_VERSION.to_owned(),
            build_id: build_id.to_owned(),
            project_id: state.project_id.clone(),
            contract_id: None,
            contract_hash: "contract".to_owned(),
            source_revision: state.revision,
            kind: crate::build::BuildKind::Draft,
            width: 960,
            height: 544,
            fps: 24,
            expected_duration_seconds: 5,
            inputs: vec![crate::build::BuildInput {
                shot_id: "S01".to_owned(),
                take_id: first.take_id.clone(),
                media_path: first.media_path.clone(),
                profile: first.profile.clone(),
                input_hash: first.input_hash.clone(),
                adapter_fingerprint: first.adapter_fingerprint.clone(),
                workflow_hash: first.workflow_hash.clone(),
                model_fingerprint: first.model_fingerprint.clone(),
                seed: first.seed,
                reference_subjects: Vec::new(),
                reference_fingerprint: String::new(),
                warnings: Vec::new(),
                first_frame_path: None,
                trim_seconds: None,
            }],
            subtitles: None,
            output_path: PathBuf::from("builds/BLD-selection/output.mp4"),
            delivery_path: PathBuf::from("review/draft-cut.mp4"),
        },
    )
    .unwrap();
    state.builds.insert(
        build_id.to_owned(),
        crate::domain::BuildRecord {
            build_id: build_id.to_owned(),
            kind: "draft".to_owned(),
            status: "needs_review".to_owned(),
            recipe: recipe_path.to_string_lossy().into_owned(),
            command_id: "build".to_owned(),
            output_path: Some(PathBuf::from("builds/BLD-selection/output.mp4")),
            warnings: Vec::new(),
            stale: false,
        },
    );
    state.pending_approvals.push(Approval {
        approval_id: "APR-build-selection".to_owned(),
        kind: ApprovalKind::BuildReview,
        subject_id: Some(build_id.to_owned()),
        shot_id: None,
        take_ids: Vec::new(),
        blocking: true,
        description: "Review draft build".to_owned(),
        created_at: "103".to_owned(),
    });
    state.bump_revision("103".to_owned()).unwrap();
    store.save_state(&state, 3).unwrap();

    let changed = store
        .select_take("S01", &second_take_id, 4, "select-second", "104")
        .unwrap();

    assert!(changed.builds[build_id].stale);
    assert!(
        changed
            .pending_approvals
            .iter()
            .all(|approval| approval.subject_id.as_deref() != Some(build_id))
    );
    assert_eq!(
        changed.project_outcome,
        crate::domain::ProjectOutcome::InProgress
    );
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
        smoke_test: false,
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

#[test]
fn dialogue_only_contract_change_keeps_raw_takes_and_stales_builds() {
    let directory = tempfile::tempdir().unwrap();
    let mut bundle = validate_json(BUNDLE).bundle.unwrap();
    let store = ProjectStore::create(
        directory.path(),
        &bundle.project.id,
        &bundle.project.title,
        "brief",
        "create",
        "100",
    )
    .unwrap();
    let (_, approval) = store.apply_bundle(&bundle, 1, "apply", "101").unwrap();
    let mut state = store
        .approve_script(Some(&approval.approval_id), 2, "approve", "102")
        .unwrap();
    let take_id = "TAKE-dialogue";
    state
        .takes
        .insert(take_id.to_owned(), take(take_id, "S01", false));
    state
        .shots
        .get_mut("S01")
        .unwrap()
        .take_ids
        .push(take_id.to_owned());
    let build_id = "BLD-dialogue";
    let recipe_path = PathBuf::from("builds/BLD-dialogue/recipe.json");
    write_json_atomic(
        &store.root().join(&recipe_path),
        &crate::build::BuildRecipe {
            schema_version: crate::build::BUILD_RECIPE_SCHEMA_VERSION.to_owned(),
            build_id: build_id.to_owned(),
            project_id: state.project_id.clone(),
            contract_id: state.active_contract_id.clone(),
            contract_hash: state.contracts[state.active_contract_id.as_ref().unwrap()]
                .bundle_hash
                .clone(),
            source_revision: state.revision,
            kind: crate::build::BuildKind::Draft,
            width: 960,
            height: 544,
            fps: 24,
            expected_duration_seconds: 5,
            inputs: vec![crate::build::BuildInput {
                shot_id: "S01".to_owned(),
                take_id: take_id.to_owned(),
                media_path: PathBuf::from("raw/S01/TAKE-dialogue.mp4"),
                profile: "audition".to_owned(),
                input_hash: "input".to_owned(),
                adapter_fingerprint: "adapter".to_owned(),
                workflow_hash: "workflow".to_owned(),
                model_fingerprint: "model".to_owned(),
                seed: 1,
                reference_subjects: Vec::new(),
                reference_fingerprint: String::new(),
                warnings: Vec::new(),
                first_frame_path: None,
                trim_seconds: None,
            }],
            subtitles: None,
            output_path: PathBuf::from("builds/BLD-dialogue/output.mp4"),
            delivery_path: PathBuf::from("review/draft-cut.mp4"),
        },
    )
    .unwrap();
    state.builds.insert(
        build_id.to_owned(),
        crate::domain::BuildRecord {
            build_id: build_id.to_owned(),
            kind: "draft".to_owned(),
            status: "needs_review".to_owned(),
            recipe: recipe_path.to_string_lossy().into_owned(),
            command_id: "build".to_owned(),
            output_path: Some(PathBuf::from("builds/BLD-dialogue/output.mp4")),
            warnings: Vec::new(),
            stale: false,
        },
    );
    state.bump_revision("103".to_owned()).unwrap();
    store.save_state(&state, 3).unwrap();

    bundle.shots[0].dialogue[0].text = "替换后的字幕对白。".to_owned();
    let (_, approval) = store
        .apply_bundle(&bundle, 4, "apply-dialogue", "104")
        .unwrap();
    let changed = store
        .approve_script(Some(&approval.approval_id), 5, "approve-dialogue", "105")
        .unwrap();

    assert!(!changed.takes[take_id].stale);
    assert!(!changed.shots["S01"].stale);
    assert!(changed.builds[build_id].stale);
}

#[test]
fn replacement_bundle_cannot_be_approved_while_a_build_is_running() {
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
    store
        .approve_script(
            Some(&initial_approval.approval_id),
            2,
            "approve-initial",
            "102",
        )
        .unwrap();
    store
        .start_build(
            crate::domain::BuildRecord {
                build_id: "BLD-running".to_owned(),
                kind: "draft".to_owned(),
                status: "queued".to_owned(),
                recipe: "builds/BLD-running/recipe.json".to_owned(),
                command_id: "build".to_owned(),
                output_path: None,
                warnings: Vec::new(),
                stale: false,
            },
            3,
            "build",
            "103",
        )
        .unwrap();
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

    assert!(matches!(error, StoreError::BuildBusy { .. }));
}

#[test]
fn build_review_gate_controls_quality_and_project_completion() {
    for (kind, approval_kind, expected_stage, expected_quality, expected_outcome) in [
        (
            "draft",
            ApprovalKind::BuildReview,
            ProjectStage::Shooting,
            crate::domain::QualityTarget::DraftCut,
            crate::domain::ProjectOutcome::InProgress,
        ),
        (
            "final",
            ApprovalKind::FinalVisualReview,
            ProjectStage::Completed,
            crate::domain::QualityTarget::Playable,
            crate::domain::ProjectOutcome::Done,
        ),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let store =
            ProjectStore::create(directory.path(), "demo", "Demo", "Brief", "create", "100")
                .unwrap();
        let build_id = format!("BLD-{kind}");
        let queued = store
            .start_build(
                crate::domain::BuildRecord {
                    build_id: build_id.clone(),
                    kind: kind.to_owned(),
                    status: "queued".to_owned(),
                    recipe: format!("builds/{build_id}/recipe.json"),
                    command_id: "build".to_owned(),
                    output_path: None,
                    warnings: Vec::new(),
                    stale: false,
                },
                1,
                "build",
                "101",
            )
            .unwrap();
        assert_eq!(queued.builds[&build_id].status, "queued");
        let running = store
            .mark_build_running(&build_id, queued.revision, "build", "102")
            .unwrap();
        assert_eq!(running.builds[&build_id].status, "running");
        let output = store.root().join(format!("builds/{build_id}/output.mp4"));
        fs::create_dir_all(output.parent().unwrap()).unwrap();
        fs::write(&output, b"video").unwrap();
        let review = store
            .finish_build(
                &build_id,
                Some(PathBuf::from(format!("builds/{build_id}/output.mp4"))),
                None,
                false,
                running.revision,
                "build",
                "103",
            )
            .unwrap();
        let approval = review
            .pending_approvals
            .iter()
            .find(|approval| approval.subject_id.as_deref() == Some(build_id.as_str()))
            .unwrap();
        assert_eq!(approval.kind, approval_kind);
        assert_eq!(
            review.project_outcome,
            crate::domain::ProjectOutcome::NeedsReview
        );

        let approved = store
            .approve_build_review(
                &approval.approval_id,
                review.revision,
                "approve-build",
                "104",
            )
            .unwrap();

        assert_eq!(approved.builds[&build_id].status, "approved");
        assert_eq!(approved.project_stage, expected_stage);
        assert_eq!(approved.quality_target, expected_quality);
        assert_eq!(approved.project_outcome, expected_outcome);
    }
}
