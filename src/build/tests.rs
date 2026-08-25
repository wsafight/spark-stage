use std::sync::mpsc::TryRecvError;

use super::*;
use crate::domain::{ProjectState, Risk, ShotRuntimeState, ShotStage, TakeMetadata};

const BUNDLE: &str = include_str!("../../skills/screenwriter/examples/valid-short-drama.json");

fn planning_fixture(profile: &str) -> (ProjectState, ScriptBundle) {
    let bundle: ScriptBundle = serde_json::from_str(BUNDLE).unwrap();
    let mut state = ProjectState::new(
        bundle.project.id.clone(),
        bundle.project.title.clone(),
        "100".to_owned(),
    );
    for (index, shot) in bundle.shots.iter().enumerate() {
        let take_id = format!("TAKE-{index:026}");
        state.takes.insert(
            take_id.clone(),
            TakeMetadata {
                take_id: take_id.clone(),
                shot_id: shot.id.clone(),
                job_id: format!("JOB-{index:026}"),
                profile: profile.to_owned(),
                status: "candidate".to_owned(),
                media_path: PathBuf::from(format!("raw/{}/{take_id}.mp4", shot.id)),
                input_hash: "input".to_owned(),
                adapter_fingerprint: "adapter".to_owned(),
                workflow_hash: "workflow".to_owned(),
                model_fingerprint: "model".to_owned(),
                seed: u64::try_from(index).unwrap(),
                elapsed_milliseconds: 100,
                first_frame_path: Some(PathBuf::from(format!(
                    "review/{}/{}-first.jpg",
                    shot.id, take_id
                ))),
                last_frame_path: None,
                handoff_candidate_path: None,
                parent_take_id: None,
                promotion_strategy: None,
                hard_checks: Vec::new(),
                warnings: Vec::new(),
                stale: false,
            },
        );
        state.shots.insert(
            shot.id.clone(),
            ShotRuntimeState {
                shot_id: shot.id.clone(),
                title: shot.title.clone(),
                stage: ShotStage::Selected,
                risk: Risk::Low,
                active_job_id: None,
                audition_target_takes: None,
                selected_candidate_take_id: Some(take_id.clone()),
                approved_take_id: (profile == shot.generation_plan.final_profile)
                    .then_some(take_id.clone()),
                take_ids: vec![take_id],
                rejected_take_ids: Vec::new(),
                fail_codes: Vec::new(),
                stale: false,
            },
        );
    }
    (state, bundle)
}

#[test]
fn trailer_filter_trims_each_input_before_concat() {
    let recipe = BuildRecipe {
        schema_version: BUILD_RECIPE_SCHEMA_VERSION.to_owned(),
        build_id: "BLD-test".to_owned(),
        project_id: "demo".to_owned(),
        contract_id: None,
        contract_hash: "contract".to_owned(),
        source_revision: 1,
        kind: BuildKind::Trailer,
        width: 960,
        height: 544,
        fps: 24,
        expected_duration_seconds: 4,
        inputs: vec![
            BuildInput {
                shot_id: "S01".to_owned(),
                take_id: "T01".to_owned(),
                media_path: PathBuf::from("raw/S01/T01.mp4"),
                profile: "final".to_owned(),
                input_hash: "input-1".to_owned(),
                adapter_fingerprint: "adapter".to_owned(),
                workflow_hash: "workflow".to_owned(),
                model_fingerprint: "model".to_owned(),
                seed: 1,
                warnings: Vec::new(),
                first_frame_path: None,
                trim_seconds: Some(2),
            },
            BuildInput {
                shot_id: "S02".to_owned(),
                take_id: "T02".to_owned(),
                media_path: PathBuf::from("raw/S02/T02.mp4"),
                profile: "final".to_owned(),
                input_hash: "input-2".to_owned(),
                adapter_fingerprint: "adapter".to_owned(),
                workflow_hash: "workflow".to_owned(),
                model_fingerprint: "model".to_owned(),
                seed: 2,
                warnings: Vec::new(),
                first_frame_path: None,
                trim_seconds: Some(2),
            },
        ],
        output_path: PathBuf::from("builds/BLD-test/output.mp4"),
        delivery_path: PathBuf::from("final/demo-trailer.mp4"),
    };

    let graph = filter_graph(&recipe);

    assert_eq!(graph.matches("trim=duration=2").count(), 4);
    assert!(graph.contains("concat=n=2:v=1:a=1[vout][aout]"));
    assert!(!graph.contains("raw/S01"));
}

#[test]
fn output_report_requires_hard_checks_dimensions_and_frame_rate() {
    let (state, bundle) = planning_fixture("audition");
    let recipe = plan("BLD-test", BuildKind::Draft, &state, &bundle).unwrap();
    let mut report = MediaReport {
        valid: true,
        duration_seconds: f64::from(recipe.expected_duration_seconds),
        fps: f64::from(recipe.fps),
        width: recipe.width,
        height: recipe.height,
        audio_channels: Some(2),
        checks: vec![crate::media::MediaCheck {
            code: "DECODE_OK".to_owned(),
            status: MediaCheckStatus::Pass,
            detail: "ok".to_owned(),
        }],
    };
    validate_output_report(&recipe, &report).unwrap();

    report.width -= 1;
    let error = validate_output_report(&recipe, &report).unwrap_err();
    assert!(error.to_string().contains("FRAME_SIZE"));

    report.width = recipe.width;
    report.checks[0].status = MediaCheckStatus::Fail;
    let error = validate_output_report(&recipe, &report).unwrap_err();
    assert!(error.to_string().contains("DECODE_OK"));
}

#[test]
fn draft_recipe_uses_only_explicit_selected_takes() {
    let (state, bundle) = planning_fixture("audition");

    let recipe = plan("BLD-test", BuildKind::Draft, &state, &bundle).unwrap();

    assert_eq!(recipe.source_revision, state.revision);
    assert_eq!(
        recipe.contract_hash,
        crate::store::sha256_json(&bundle).unwrap()
    );
    assert_eq!(recipe.inputs.len(), bundle.shots.len());
    assert_eq!(
        recipe.expected_duration_seconds,
        bundle.project.target_duration_seconds
    );
    assert!(
        recipe
            .inputs
            .iter()
            .all(|input| input.trim_seconds.is_none())
    );
    assert!(
        recipe
            .inputs
            .iter()
            .all(|input| input.adapter_fingerprint == "adapter"
                && input.workflow_hash == "workflow"
                && input.model_fingerprint == "model")
    );
    validate_current(&recipe, &state).unwrap();
}

#[test]
fn scoped_draft_uses_contract_order_and_rejects_invalid_selection() {
    let (state, bundle) = planning_fixture("audition");

    let recipe = plan_selected(
        "BLD-scoped",
        BuildKind::Draft,
        &state,
        &bundle,
        &["S02".to_owned()],
    )
    .unwrap();

    assert_eq!(recipe.inputs.len(), 1);
    assert_eq!(recipe.inputs[0].shot_id, "S02");
    assert_eq!(recipe.expected_duration_seconds, bundle.shots[1].duration);
    validate_current(&recipe, &state).unwrap();
    assert!(matches!(
        plan_selected(
            "BLD-duplicate",
            BuildKind::Draft,
            &state,
            &bundle,
            &["S01".to_owned(), "S01".to_owned()]
        ),
        Err(BuildError::DuplicateShotSelection)
    ));
    assert!(matches!(
        plan_selected(
            "BLD-missing",
            BuildKind::Draft,
            &state,
            &bundle,
            &["S99".to_owned()]
        ),
        Err(BuildError::ShotSelectionMissing(id)) if id == "S99"
    ));
}

#[test]
fn current_validation_rejects_changed_take_decision() {
    let (mut state, bundle) = planning_fixture("audition");
    let recipe = plan("BLD-test", BuildKind::Draft, &state, &bundle).unwrap();
    state
        .shots
        .get_mut("S01")
        .unwrap()
        .selected_candidate_take_id = Some("TAKE-other".to_owned());

    let error = validate_current(&recipe, &state).unwrap_err();

    assert!(matches!(error, BuildError::Stale(_)));
}

#[test]
fn build_planning_requires_a_safe_extracted_first_frame() {
    let (mut state, bundle) = planning_fixture("audition");
    let take_id = state.shots["S01"]
        .selected_candidate_take_id
        .clone()
        .unwrap();
    state.takes.get_mut(&take_id).unwrap().first_frame_path = None;
    assert!(matches!(
        plan("BLD-test", BuildKind::Draft, &state, &bundle),
        Err(BuildError::FirstFrameMissing(id)) if id == take_id
    ));

    state.takes.get_mut(&take_id).unwrap().first_frame_path = Some(PathBuf::from("../outside.jpg"));
    assert!(matches!(
        plan("BLD-test", BuildKind::Draft, &state, &bundle),
        Err(BuildError::Path(_))
    ));
}

#[test]
fn final_recipe_rejects_audition_profile_even_if_marked_approved() {
    let (mut state, bundle) = planning_fixture("audition");
    for shot in state.shots.values_mut() {
        shot.approved_take_id = shot.selected_candidate_take_id.clone();
    }

    let error = plan("BLD-test", BuildKind::Final, &state, &bundle).unwrap_err();

    assert!(matches!(error, BuildError::FinalProfileRequired { .. }));
}

#[test]
fn recipe_rejects_cross_shot_and_unsafe_take_paths() {
    let (mut state, bundle) = planning_fixture("audition");
    let take_id = state.shots["S01"]
        .selected_candidate_take_id
        .clone()
        .unwrap();
    state.takes.get_mut(&take_id).unwrap().shot_id = "S02".to_owned();
    assert!(matches!(
        plan("BLD-test", BuildKind::Draft, &state, &bundle),
        Err(BuildError::TakeShotMismatch { .. })
    ));

    state.takes.get_mut(&take_id).unwrap().shot_id = "S01".to_owned();
    state.takes.get_mut(&take_id).unwrap().media_path = PathBuf::from("../outside.mp4");
    assert!(matches!(
        plan("BLD-test", BuildKind::Draft, &state, &bundle),
        Err(BuildError::Path(_))
    ));
}

#[test]
fn capability_listing_handles_combined_format_names() {
    assert!(listing_contains(" D  mov,mp4,m4a,3gp", "mp4"));
    assert!(!listing_contains(" V..... h264_videotoolbox", "libx264"));
}

#[test]
fn review_report_serializes_recipe_lineage_and_contact_sheet() {
    let (state, bundle) = planning_fixture("audition");
    let recipe = plan("BLD-review", BuildKind::Draft, &state, &bundle).unwrap();
    let expected_input = recipe.inputs[0].clone();
    let report = BuildReviewReport {
        schema_version: BUILD_REVIEW_REPORT_SCHEMA_VERSION.to_owned(),
        build_id: recipe.build_id.clone(),
        project_id: recipe.project_id.clone(),
        kind: recipe.kind,
        recipe_path: PathBuf::from("builds/BLD-review/recipe.json"),
        output_path: recipe.output_path.clone(),
        delivery_path: recipe.delivery_path.clone(),
        contact_sheet_path: PathBuf::from("builds/BLD-review/contact-sheet.jpg"),
        recipe,
        media: MediaReport {
            valid: true,
            duration_seconds: 5.0,
            fps: 24.0,
            width: 960,
            height: 544,
            audio_channels: Some(2),
            checks: Vec::new(),
        },
        human_visual_review_required: true,
    };

    let value = serde_json::to_value(report).unwrap();
    assert_eq!(
        value["contact_sheet_path"],
        "builds/BLD-review/contact-sheet.jpg"
    );
    assert_eq!(
        value["recipe"]["inputs"][0]["take_id"],
        expected_input.take_id
    );
    assert_eq!(
        value["recipe"]["inputs"][0]["input_hash"],
        expected_input.input_hash
    );
    assert_eq!(
        value["recipe"]["inputs"][0]["workflow_hash"],
        expected_input.workflow_hash
    );
    assert_eq!(value["recipe"]["inputs"][0]["seed"], expected_input.seed);
    assert_eq!(value["human_visual_review_required"], true);
}

#[test]
fn executor_reports_started_before_terminal_event() {
    let directory = tempfile::tempdir().unwrap();
    let (state, bundle) = planning_fixture("audition");
    let mut recipe = plan("BLD-events", BuildKind::Draft, &state, &bundle).unwrap();
    recipe.inputs.clear();
    let executor = BuildExecutorHandle::spawn().unwrap();
    executor
        .send(BuildRequest {
            project_id: state.project_id,
            project_root: directory.path().to_owned(),
            command_id: "build-command".to_owned(),
            recipe,
        })
        .unwrap();

    let receive = || {
        for _ in 0..100 {
            match executor.try_recv() {
                Ok(event) => return event,
                Err(TryRecvError::Empty) => {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                Err(TryRecvError::Disconnected) => panic!("build executor disconnected"),
            }
        }
        panic!("build executor event timed out")
    };

    assert!(matches!(receive(), BuildEvent::Started(_)));
    assert!(matches!(receive(), BuildEvent::Failed { .. }));
}
