use super::*;

#[test]
fn executor_events_persist_attempt_and_complete_candidate() {
    let directory = tempfile::tempdir().unwrap();
    let paths = AppPaths::resolve(Some(directory.path().join("data")), None);
    let adapter = verified_adapter(directory.path());
    let mut runtime = WorkerRuntime::open_with_adapter(paths.clone(), Some(adapter)).unwrap();
    approved_project(&mut runtime);
    let queued = runtime.handle(request(
        Some("rain-apartment"),
        Some(3),
        WorkerCommand::AuditionShot {
            shot_id: "S01".to_owned(),
        },
    ));
    let job_id = queued.snapshot.unwrap().queue.jobs[0].job_id.clone();
    let ExecutorRequest::Prepare(context) = runtime.next_executor_request().unwrap().unwrap()
    else {
        panic!("queued job should start with preparation");
    };
    let request_id = context.job.attempts[0].request_id.clone();
    let store = ProjectStore::open(&paths.projects_dir, "rain-apartment").unwrap();
    let active = store.read_job(&job_id).unwrap();
    assert_eq!(active.state, JobState::Active);
    assert_eq!(active.attempts[0].state, AttemptState::Prepared);
    assert_eq!(
        store.read_state().unwrap().shots["S01"].stage,
        ShotStage::Generating
    );

    let prepared = crate::adapter::PreparedJob {
        request_id: request_id.clone(),
        client_id: "client-1".to_owned(),
        workflow_hash: "workflow-1".to_owned(),
        output_node: "3".to_owned(),
        output_prefix: "sparkstage/test".to_owned(),
        workflow: serde_json::json!({}),
    };
    let next = runtime
        .apply_executor_event(ExecutorEvent::Prepared {
            context: Box::new(context),
            prepared: Box::new(prepared),
        })
        .unwrap();
    assert!(matches!(next, Some(ExecutorRequest::Submit { .. })));
    assert_eq!(
        store.read_job(&job_id).unwrap().attempts[0].state,
        AttemptState::Submitting
    );

    let project_revision = store.read_state().unwrap().revision;
    let queue_revision = runtime.queue.revision;
    let (mut subscriber_client, subscriber_server) = UnixStream::pair().unwrap();
    subscriber_client
        .set_read_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    write_frame(
        &mut subscriber_client,
        &request(
            Some("rain-apartment"),
            None,
            WorkerCommand::Subscribe {
                project_revision,
                queue_revision,
            },
        ),
    )
    .unwrap();
    let mut subscribers = Vec::new();
    serve_connection(&mut runtime, subscriber_server, &mut subscribers).unwrap();
    let acknowledgement: WorkerReply = read_frame(&mut subscriber_client).unwrap();
    assert!(acknowledgement.ok, "{acknowledgement:?}");

    runtime
        .apply_executor_event(ExecutorEvent::Submitted {
            job_id: job_id.clone(),
            request_id: request_id.clone(),
            backend_job_id: crate::adapter::BackendJobId("backend-1".to_owned()),
        })
        .unwrap();
    assert_eq!(runtime.queue.revision, queue_revision + 1);
    notify_subscribers(&runtime, &mut subscribers);
    let revision_event: RevisionEvent = read_frame(&mut subscriber_client).unwrap();
    assert_eq!(revision_event.project_revision, project_revision);
    assert_eq!(revision_event.queue_revision, queue_revision + 1);
    let submitted = store.read_job(&job_id).unwrap();
    assert_eq!(submitted.attempts[0].state, AttemptState::Submitted);
    assert_eq!(
        submitted.attempts[0].backend_job_id.as_deref(),
        Some("backend-1")
    );

    runtime
        .apply_executor_event(ExecutorEvent::RetryableMonitorError {
            job_id: job_id.clone(),
            request_id: request_id.clone(),
            message: "temporary websocket disconnect".to_owned(),
        })
        .unwrap();
    assert_eq!(
        store.read_job(&job_id).unwrap().attempts[0].state,
        AttemptState::Running
    );
    assert!(matches!(
        runtime.next_executor_request().unwrap(),
        Some(ExecutorRequest::Reconcile { .. })
    ));

    let raw = store.root().join("raw/S01/candidate.mp4");
    let review = store.root().join("review/S01");
    fs::create_dir_all(&review).unwrap();
    fs::create_dir_all(raw.parent().unwrap()).unwrap();
    fs::write(&raw, b"video").unwrap();
    let boundaries = crate::media::BoundaryFrames {
        first: review.join("first.jpg"),
        last: review.join("last.jpg"),
        handoff_candidate: review.join("handoff.jpg"),
    };
    for path in [
        &boundaries.first,
        &boundaries.last,
        &boundaries.handoff_candidate,
    ] {
        fs::write(path, b"frame").unwrap();
    }
    runtime
        .apply_executor_event(ExecutorEvent::Completed {
            job_id: job_id.clone(),
            request_id,
            workflow_hash: "workflow-1".to_owned(),
            model_fingerprint: "model-1".to_owned(),
            media_path: raw,
            report: crate::media::MediaReport {
                valid: true,
                duration_seconds: 5.0,
                fps: 24.0,
                width: 960,
                height: 544,
                audio_channels: Some(2),
                checks: Vec::new(),
            },
            boundaries,
            elapsed_milliseconds: 42,
        })
        .unwrap();

    let completed = store.read_job(&job_id).unwrap();
    assert_eq!(completed.state, JobState::Completed);
    assert_eq!(completed.attempts[0].state, AttemptState::Completed);
    let state = store.read_state().unwrap();
    let take_id = &completed.reserved_take_id;
    assert!(state.takes.contains_key(take_id));
    assert_eq!(state.shots["S01"].stage, ShotStage::Queued);
    assert!(state.shots["S01"].active_job_id.is_some());
    assert_eq!(state.shots["S01"].audition_target_takes, Some(3));
    assert!(state.pending_approvals.iter().any(|approval| {
        approval.kind == ApprovalKind::CandidateSelection && approval.take_ids.contains(take_id)
    }));
    assert!(runtime.queue.running.is_none());
    assert_eq!(runtime.queue.pending.len(), 1);
}

#[test]
fn submission_unknown_blocks_gpu_dispatch_without_resubmitting() {
    let directory = tempfile::tempdir().unwrap();
    let paths = AppPaths::resolve(Some(directory.path().join("data")), None);
    let adapter = verified_adapter(directory.path());
    let mut runtime = WorkerRuntime::open_with_adapter(paths.clone(), Some(adapter)).unwrap();
    approved_project(&mut runtime);
    let queued = runtime.handle(request(
        Some("rain-apartment"),
        Some(3),
        WorkerCommand::AuditionShot {
            shot_id: "S01".to_owned(),
        },
    ));
    let job_id = queued.snapshot.unwrap().queue.jobs[0].job_id.clone();
    let ExecutorRequest::Prepare(context) = runtime.next_executor_request().unwrap().unwrap()
    else {
        panic!("queued job should start with preparation");
    };
    let request_id = context.job.attempts[0].request_id.clone();

    runtime
        .apply_executor_event(ExecutorEvent::SubmissionUnknown {
            job_id: job_id.clone(),
            request_id,
            message: "connection closed after POST".to_owned(),
        })
        .unwrap();

    let store = ProjectStore::open(&paths.projects_dir, "rain-apartment").unwrap();
    let blocked = store.read_job(&job_id).unwrap();
    assert_eq!(blocked.state, JobState::Blocked);
    assert_eq!(blocked.attempts[0].state, AttemptState::SubmissionUnknown);
    assert_eq!(
        blocked.attempts[0].error_code.as_deref(),
        Some("SUBMISSION_UNKNOWN")
    );
    assert!(runtime.next_executor_request().unwrap().is_none());
    assert_eq!(runtime.queue.running.as_ref().unwrap().job_id, job_id);
}

#[test]
fn startup_recovers_completed_job_and_resumes_audition_batch() {
    let directory = tempfile::tempdir().unwrap();
    let paths = AppPaths::resolve(Some(directory.path().join("data")), None);
    let adapter = verified_adapter(directory.path());
    let mut runtime =
        WorkerRuntime::open_with_adapter(paths.clone(), Some(adapter.clone())).unwrap();
    approved_project(&mut runtime);
    let queued = runtime.handle(request(
        Some("rain-apartment"),
        Some(3),
        WorkerCommand::AuditionShot {
            shot_id: "S01".to_owned(),
        },
    ));
    let job_id = queued.snapshot.unwrap().queue.jobs[0].job_id.clone();
    let _ = runtime.next_executor_request().unwrap().unwrap();
    let store = ProjectStore::open(&paths.projects_dir, "rain-apartment").unwrap();
    let mut job = store.read_job(&job_id).unwrap();
    let take = TakeMetadata {
        take_id: job.reserved_take_id.clone(),
        shot_id: job.shot_id.clone(),
        job_id: job.job_id.clone(),
        profile: job.profile.clone(),
        status: "candidate".to_owned(),
        media_path: PathBuf::from(format!("raw/S01/{}.mp4", job.reserved_take_id)),
        input_hash: job.input_hash.clone(),
        adapter_fingerprint: job.adapter_fingerprint.clone(),
        workflow_hash: "workflow-1".to_owned(),
        model_fingerprint: "model-1".to_owned(),
        seed: job.seed,
        elapsed_milliseconds: 42,
        first_frame_path: None,
        last_frame_path: None,
        handoff_candidate_path: None,
        parent_take_id: None,
        promotion_strategy: None,
        hard_checks: Vec::new(),
        warnings: Vec::new(),
        stale: false,
    };
    store.save_take_metadata(&take).unwrap();
    job.state = JobState::Completed;
    job.attempts[0].state = AttemptState::Completed;
    store.save_job(&job).unwrap();
    drop(runtime);

    let recovered = WorkerRuntime::open_with_adapter(paths, Some(adapter)).unwrap();

    let state = store.read_state().unwrap();
    assert!(state.shots["S01"].active_job_id.is_some());
    assert_eq!(state.shots["S01"].stage, ShotStage::Queued);
    assert!(state.takes.contains_key(&take.take_id));
    assert!(state.pending_approvals.iter().any(|approval| {
        approval.kind == ApprovalKind::CandidateSelection
            && approval.take_ids.contains(&take.take_id)
    }));
    let resumed_job = store
        .read_job(state.shots["S01"].active_job_id.as_deref().unwrap())
        .unwrap();
    let bundle = store.read_active_bundle().unwrap().unwrap();
    let shot = bundle
        .shots
        .iter()
        .find(|shot| shot.id == resumed_job.shot_id)
        .unwrap();
    let reference_subjects = crate::store::reference_subject_keys(shot);
    let reference_fingerprint =
        crate::store::active_reference_fingerprint(&state, &reference_subjects);
    let expected_input_hash = sha256_json(&(
        resumed_job.contract_id.as_str(),
        shot,
        &resumed_job.profile,
        resumed_job.seed,
        Option::<&str>::None,
        Option::<PromotionStrategy>::None,
        reference_fingerprint,
    ))
    .unwrap();
    assert_eq!(resumed_job.input_hash, expected_input_hash);
    assert!(recovered.queue.running.is_none());
    assert_eq!(recovered.queue.pending.len(), 1);
}

#[test]
fn adapter_fingerprint_change_is_persisted_as_dispatch_block() {
    let directory = tempfile::tempdir().unwrap();
    let paths = AppPaths::resolve(Some(directory.path().join("data")), None);
    let adapter = verified_adapter(directory.path());
    let mut runtime = WorkerRuntime::open_with_adapter(paths.clone(), Some(adapter)).unwrap();
    approved_project(&mut runtime);
    let queued = runtime.handle(request(
        Some("rain-apartment"),
        Some(3),
        WorkerCommand::AuditionShot {
            shot_id: "S01".to_owned(),
        },
    ));
    let job_id = queued.snapshot.unwrap().queue.jobs[0].job_id.clone();
    let workflow_path = directory.path().join("workflow.json");
    let mut workflow: serde_json::Value = crate::store::read_json(&workflow_path).unwrap();
    workflow["4"] = serde_json::json!({"class_type": "Noop", "inputs": {}});
    write_json_atomic(&workflow_path, &workflow).unwrap();

    assert!(runtime.next_executor_request().unwrap().is_none());

    let store = ProjectStore::open(&paths.projects_dir, "rain-apartment").unwrap();
    let blocked = store.read_job(&job_id).unwrap();
    assert_eq!(blocked.state, JobState::Blocked);
    assert_eq!(
        blocked.attempts[0].error_code.as_deref(),
        Some("DISPATCH_BLOCKED")
    );
    assert!(runtime.next_executor_request().unwrap().is_none());
}
