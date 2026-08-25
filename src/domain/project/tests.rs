use super::*;

fn state() -> ProjectState {
    ProjectState::new("demo".to_owned(), "Demo".to_owned(), "1".to_owned())
}

fn shot(shot_id: &str) -> ShotRuntimeState {
    ShotRuntimeState {
        shot_id: shot_id.to_owned(),
        title: format!("Shot {shot_id}"),
        stage: ShotStage::Pending,
        risk: Risk::Low,
        active_job_id: None,
        audition_target_takes: None,
        selected_candidate_take_id: None,
        approved_take_id: None,
        take_ids: Vec::new(),
        rejected_take_ids: Vec::new(),
        fail_codes: Vec::new(),
        stale: false,
    }
}

fn approval(id: &str, kind: ApprovalKind, blocking: bool) -> Approval {
    Approval {
        approval_id: id.to_owned(),
        kind,
        subject_id: None,
        shot_id: None,
        take_ids: Vec::new(),
        blocking,
        description: "Review required".to_owned(),
        created_at: "1".to_owned(),
    }
}

#[test]
fn blocking_approval_derives_needs_review() {
    let mut state = state();
    let mut approval = approval("APR-1", ApprovalKind::ScriptBundle, true);
    approval.subject_id = Some("CON-1".to_owned());
    state.pending_approvals.push(approval);

    state.bump_revision("2".to_owned()).unwrap();

    assert_eq!(state.project_outcome, ProjectOutcome::NeedsReview);
    assert_eq!(state.revision, 2);
}

#[test]
fn active_contract_must_exist() {
    let mut state = state();
    state.active_contract_id = Some("missing".to_owned());

    assert_eq!(
        state.validate(),
        Err(StateInvariantError::MissingActiveContract)
    );
}

#[test]
fn shot_take_references_must_resolve() {
    let mut state = state();
    let mut shot = shot("S01");
    shot.stage = ShotStage::CandidatesReady;
    shot.take_ids.push("TAKE-missing".to_owned());
    state.shots.insert("S01".to_owned(), shot);

    assert!(matches!(
        state.validate(),
        Err(StateInvariantError::ShotTakeMissing { .. })
    ));
}

#[test]
fn schema_is_validated_before_relationships() {
    let mut unsupported = state();
    unsupported.schema_version = "999.0".to_owned();
    unsupported.active_contract_id = Some("missing".to_owned());
    assert_eq!(
        unsupported.validate(),
        Err(StateInvariantError::UnsupportedSchema("999.0".to_owned()))
    );
}

#[test]
fn revision_is_validated_before_relationships() {
    let mut zero_revision = state();
    zero_revision.revision = 0;
    zero_revision.active_contract_id = Some("missing".to_owned());
    assert_eq!(
        zero_revision.validate(),
        Err(StateInvariantError::ZeroRevision)
    );
}

#[test]
fn project_outcome_must_match_blocking_approvals() {
    let mut state = state();
    state
        .pending_approvals
        .push(approval("APR-1", ApprovalKind::BudgetOverrun, true));

    assert_eq!(
        state.validate(),
        Err(StateInvariantError::OutcomeApprovalMismatch)
    );
}

#[test]
fn approval_ids_must_be_unique() {
    let mut state = state();
    state
        .pending_approvals
        .push(approval("APR-duplicate", ApprovalKind::ScriptBundle, false));
    state
        .pending_approvals
        .push(approval("APR-duplicate", ApprovalKind::BuildReview, false));

    assert_eq!(
        state.validate(),
        Err(StateInvariantError::DuplicateApproval(
            "APR-duplicate".to_owned()
        ))
    );
}

#[test]
fn shot_map_key_must_match_embedded_id() {
    let mut state = state();
    state.shots.insert("S01".to_owned(), shot("S02"));

    assert_eq!(
        state.validate(),
        Err(StateInvariantError::ShotKeyMismatch {
            key: "S01".to_owned(),
            shot_id: "S02".to_owned()
        })
    );
}

#[test]
fn candidate_approval_requires_a_known_shot() {
    let mut missing = state();
    missing.pending_approvals.push(approval(
        "APR-candidate",
        ApprovalKind::CandidateSelection,
        false,
    ));
    assert_eq!(
        missing.validate(),
        Err(StateInvariantError::CandidateApprovalShotMissing(
            "APR-candidate".to_owned()
        ))
    );

    let mut unknown = state();
    let mut candidate = approval("APR-candidate", ApprovalKind::CandidateSelection, false);
    candidate.shot_id = Some("S99".to_owned());
    unknown.pending_approvals.push(candidate);
    assert_eq!(
        unknown.validate(),
        Err(StateInvariantError::CandidateApprovalShotUnknown {
            approval_id: "APR-candidate".to_owned(),
            shot_id: "S99".to_owned()
        })
    );
}
