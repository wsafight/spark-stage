use super::*;
use crate::store::BatchTakeSelection;

fn seed_review_candidates(paths: &AppPaths, warning_on_second: bool) -> (String, String, u64) {
    let store = ProjectStore::open(&paths.projects_dir, "rain-apartment").unwrap();
    let mut state = store.read_state().unwrap();
    let mut take_ids = Vec::new();
    for (index, shot_id) in ["S01", "S02"].into_iter().enumerate() {
        let take_id = format!("TAKE-{index:026}");
        state.takes.insert(
            take_id.clone(),
            TakeMetadata {
                take_id: take_id.clone(),
                shot_id: shot_id.to_owned(),
                job_id: format!("JOB-{index:026}"),
                profile: "audition".to_owned(),
                status: "candidate".to_owned(),
                media_path: PathBuf::from(format!("raw/{shot_id}/{take_id}.mp4")),
                input_hash: "input".to_owned(),
                adapter_fingerprint: "adapter".to_owned(),
                workflow_hash: "workflow".to_owned(),
                model_fingerprint: "model".to_owned(),
                seed: index as u64,
                elapsed_milliseconds: 10,
                first_frame_path: None,
                last_frame_path: None,
                handoff_candidate_path: None,
                parent_take_id: None,
                promotion_strategy: None,
                hard_checks: vec!["MEDIA_PROBE_OK".to_owned()],
                warnings: if warning_on_second && index == 1 {
                    vec!["AUDIO_LOUDNESS_WARNING".to_owned()]
                } else {
                    Vec::new()
                },
                stale: false,
            },
        );
        let shot = state.shots.get_mut(shot_id).unwrap();
        shot.stage = ShotStage::CandidatesReady;
        shot.take_ids.push(take_id.clone());
        take_ids.push(take_id);
    }
    state.bump_revision("seed".to_owned()).unwrap();
    store.save_state(&state, 3).unwrap();
    (take_ids.remove(0), take_ids.remove(0), state.revision)
}

fn selection(shot_id: &str, take_id: String, accept_warnings: bool) -> BatchTakeSelection {
    BatchTakeSelection {
        shot_id: shot_id.to_owned(),
        take_id,
        accept_warnings,
    }
}

#[test]
fn batch_review_selects_multiple_takes_in_one_revision() {
    let directory = tempfile::tempdir().unwrap();
    let paths = AppPaths::resolve(Some(directory.path().join("data")), None);
    let mut runtime = WorkerRuntime::open(paths.clone()).unwrap();
    approved_project(&mut runtime);
    let (first, second, revision) = seed_review_candidates(&paths, false);

    let reply = runtime.handle(request(
        Some("rain-apartment"),
        Some(revision),
        WorkerCommand::ReviewBatch {
            selections: vec![
                selection("S01", first.clone(), false),
                selection("S02", second.clone(), false),
            ],
            approve: false,
        },
    ));

    assert!(reply.ok, "{reply:?}");
    assert_eq!(reply.revision, Some(revision + 1));
    let state = ProjectStore::open(&paths.projects_dir, "rain-apartment")
        .unwrap()
        .read_state()
        .unwrap();
    assert_eq!(
        state.shots["S01"].selected_candidate_take_id.as_deref(),
        Some(first.as_str())
    );
    assert_eq!(
        state.shots["S02"].selected_candidate_take_id.as_deref(),
        Some(second.as_str())
    );
    assert!(
        state
            .shots
            .values()
            .all(|shot| shot.approved_take_id.is_none())
    );
}

#[test]
fn invalid_batch_does_not_partially_select_valid_take() {
    let directory = tempfile::tempdir().unwrap();
    let paths = AppPaths::resolve(Some(directory.path().join("data")), None);
    let mut runtime = WorkerRuntime::open(paths.clone()).unwrap();
    approved_project(&mut runtime);
    let (first, _second, revision) = seed_review_candidates(&paths, false);

    let reply = runtime.handle(request(
        Some("rain-apartment"),
        Some(revision),
        WorkerCommand::ReviewBatch {
            selections: vec![
                selection("S01", first, false),
                selection("S02", "TAKE-MISSING".to_owned(), false),
            ],
            approve: false,
        },
    ));

    assert!(!reply.ok);
    assert_eq!(reply.error.unwrap().code, "TAKE_NOT_FOUND");
    let state = ProjectStore::open(&paths.projects_dir, "rain-apartment")
        .unwrap()
        .read_state()
        .unwrap();
    assert_eq!(state.revision, revision);
    assert!(
        state
            .shots
            .values()
            .all(|shot| shot.selected_candidate_take_id.is_none())
    );
}

#[test]
fn batch_approval_requires_explicit_warning_acceptance() {
    let directory = tempfile::tempdir().unwrap();
    let paths = AppPaths::resolve(Some(directory.path().join("data")), None);
    let mut runtime = WorkerRuntime::open(paths.clone()).unwrap();
    approved_project(&mut runtime);
    let (first, second, revision) = seed_review_candidates(&paths, true);
    let selections = vec![
        selection("S01", first.clone(), false),
        selection("S02", second.clone(), false),
    ];

    let blocked = runtime.handle(request(
        Some("rain-apartment"),
        Some(revision),
        WorkerCommand::ReviewBatch {
            selections,
            approve: true,
        },
    ));
    assert!(!blocked.ok);
    assert_eq!(blocked.error.unwrap().code, "REVIEW_WARNINGS_NOT_ACCEPTED");

    let approved = runtime.handle(request(
        Some("rain-apartment"),
        Some(revision),
        WorkerCommand::ReviewBatch {
            selections: vec![
                selection("S01", first.clone(), false),
                selection("S02", second.clone(), true),
            ],
            approve: true,
        },
    ));
    assert!(approved.ok, "{approved:?}");
    let state = ProjectStore::open(&paths.projects_dir, "rain-apartment")
        .unwrap()
        .read_state()
        .unwrap();
    assert_eq!(
        state.shots["S01"].approved_take_id.as_deref(),
        Some(first.as_str())
    );
    assert_eq!(
        state.shots["S02"].approved_take_id.as_deref(),
        Some(second.as_str())
    );
}
