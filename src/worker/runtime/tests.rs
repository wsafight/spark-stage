use super::*;

const BUNDLE: &str = include_str!("../../../skills/screenwriter/examples/valid-short-drama.json");

fn request(
    project_id: Option<&str>,
    expected_revision: Option<u64>,
    command: WorkerCommand,
) -> ClientRequest {
    ClientRequest {
        protocol_version: IPC_PROTOCOL_VERSION.to_owned(),
        command_id: Ulid::new().to_string(),
        expected_revision,
        project_id: project_id.map(str::to_owned),
        command,
    }
}

fn runtime() -> (tempfile::TempDir, WorkerRuntime) {
    let directory = tempfile::tempdir().unwrap();
    let paths = AppPaths::resolve(Some(directory.path().join("data")), None);
    let runtime = WorkerRuntime::open(paths).unwrap();
    (directory, runtime)
}

#[test]
fn authoring_flow_is_worker_owned() {
    let (_directory, mut runtime) = runtime();
    let create = runtime.handle(request(
        None,
        None,
        WorkerCommand::CreateProject {
            project_id: "rain-apartment".to_owned(),
            title: "Rain Apartment".to_owned(),
            brief: "A brief".to_owned(),
        },
    ));
    assert!(create.ok, "{create:?}");
    assert_eq!(create.revision, Some(1));

    let apply = runtime.handle(request(
        Some("rain-apartment"),
        Some(1),
        WorkerCommand::ApplyScript {
            bundle_json: BUNDLE.to_owned(),
        },
    ));
    assert!(apply.ok, "{apply:?}");
    assert_eq!(apply.revision, Some(2));
    assert_eq!(apply.snapshot.as_ref().unwrap().pending_approvals.len(), 1);

    let approve = runtime.handle(request(
        Some("rain-apartment"),
        Some(2),
        WorkerCommand::ApproveScript,
    ));
    assert!(approve.ok, "{approve:?}");
    let snapshot = approve.snapshot.unwrap();
    assert_eq!(snapshot.revision, 3);
    assert_eq!(snapshot.project.stage, "shooting");
    assert_eq!(snapshot.shots.len(), 2);
    assert!(snapshot.pending_approvals.is_empty());
}

#[test]
fn duplicate_command_returns_original_reply() {
    let (_directory, mut runtime) = runtime();
    let request = request(
        None,
        None,
        WorkerCommand::CreateProject {
            project_id: "demo".to_owned(),
            title: "Demo".to_owned(),
            brief: "Brief".to_owned(),
        },
    );
    let first = runtime.handle(request.clone());
    let second = runtime.handle(request);
    assert!(first.ok);
    assert_eq!(first, second);
}

#[test]
fn stale_mutation_returns_current_revision() {
    let (_directory, mut runtime) = runtime();
    runtime.handle(request(
        None,
        None,
        WorkerCommand::CreateProject {
            project_id: "rain-apartment".to_owned(),
            title: "Rain Apartment".to_owned(),
            brief: "Brief".to_owned(),
        },
    ));
    let reply = runtime.handle(request(
        Some("rain-apartment"),
        Some(99),
        WorkerCommand::ApplyScript {
            bundle_json: BUNDLE.to_owned(),
        },
    ));
    assert!(!reply.ok);
    assert_eq!(reply.revision, Some(1));
    assert_eq!(reply.error.unwrap().code, "REVISION_CONFLICT");
}

#[test]
fn startup_recovers_prepared_command_when_project_state_was_written() {
    let directory = tempfile::tempdir().unwrap();
    let paths = AppPaths::resolve(Some(directory.path().join("data")), None);
    fs::create_dir_all(&paths.runtime_dir).unwrap();
    write_json_atomic(&paths.queue_file(), &QueueState::default()).unwrap();

    let create = request(
        None,
        None,
        WorkerCommand::CreateProject {
            project_id: "demo".to_owned(),
            title: "Demo".to_owned(),
            brief: "Brief".to_owned(),
        },
    );
    ProjectStore::create(
        &paths.projects_dir,
        "demo",
        "Demo",
        "Brief",
        &create.command_id,
        "100",
    )
    .unwrap();
    append_prepared(&paths, &create);

    let mut recovered = WorkerRuntime::open(paths).unwrap();
    let reply = recovered.handle(create);

    assert!(reply.ok, "{reply:?}");
    assert_eq!(reply.revision, Some(1));
    assert!(reply.message.unwrap().contains("recovered committed"));
}

#[test]
fn startup_aborts_prepared_command_when_state_was_not_written() {
    let directory = tempfile::tempdir().unwrap();
    let paths = AppPaths::resolve(Some(directory.path().join("data")), None);
    let mut initial = WorkerRuntime::open(paths.clone()).unwrap();
    let create = request(
        None,
        None,
        WorkerCommand::CreateProject {
            project_id: "demo".to_owned(),
            title: "Demo".to_owned(),
            brief: "Brief".to_owned(),
        },
    );
    assert!(initial.handle(create).ok);
    drop(initial);

    let apply = request(
        Some("demo"),
        Some(1),
        WorkerCommand::ApplyScript {
            bundle_json: BUNDLE.to_owned(),
        },
    );
    append_prepared(&paths, &apply);

    let mut recovered = WorkerRuntime::open(paths).unwrap();
    let reply = recovered.handle(apply);

    assert!(!reply.ok);
    assert_eq!(reply.error.unwrap().code, "COMMAND_ABORTED_BEFORE_COMMIT");
    assert_eq!(reply.revision, Some(1));
}

#[test]
fn queue_pause_is_machine_state_without_project_write() {
    let (_directory, mut runtime) = runtime();
    let create = request(
        None,
        None,
        WorkerCommand::CreateProject {
            project_id: "demo".to_owned(),
            title: "Demo".to_owned(),
            brief: "Brief".to_owned(),
        },
    );
    let create_id = create.command_id.clone();
    assert!(runtime.handle(create).ok);

    let pause = request(Some("demo"), Some(1), WorkerCommand::PauseQueue);
    let pause_id = pause.command_id.clone();
    let reply = runtime.handle(pause);

    assert!(reply.ok, "{reply:?}");
    assert_eq!(reply.revision, Some(1));
    assert!(reply.snapshot.unwrap().queue.paused);
    assert_eq!(
        runtime.queue.last_command_id.as_deref(),
        Some(pause_id.as_str())
    );
    let state = ProjectStore::open(&runtime.paths.projects_dir, "demo")
        .unwrap()
        .read_state()
        .unwrap();
    assert_eq!(state.last_command_id.as_deref(), Some(create_id.as_str()));
}

#[test]
fn startup_recovers_prepared_queue_command_from_queue_state() {
    let directory = tempfile::tempdir().unwrap();
    let paths = AppPaths::resolve(Some(directory.path().join("data")), None);
    let mut initial = WorkerRuntime::open(paths.clone()).unwrap();
    assert!(
        initial
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
    drop(initial);

    let pause = request(Some("demo"), Some(1), WorkerCommand::PauseQueue);
    append_prepared(&paths, &pause);
    let mut queue: QueueState = crate::store::read_json(&paths.queue_file()).unwrap();
    queue.paused = true;
    queue.revision += 1;
    queue.last_command_id = Some(pause.command_id.clone());
    write_json_atomic(&paths.queue_file(), &queue).unwrap();

    let mut recovered = WorkerRuntime::open(paths).unwrap();
    let reply = recovered.handle(pause);

    assert!(reply.ok, "{reply:?}");
    assert!(reply.snapshot.unwrap().queue.paused);
    assert!(reply.message.unwrap().contains("recovered committed"));
}

fn append_prepared(paths: &AppPaths, request: &ClientRequest) {
    append_jsonl(
        &paths.command_journal(),
        &CommandJournalEvent {
            event_id: format!("CJE-{}", Ulid::new()),
            command_id: request.command_id.clone(),
            request_hash: sha256_json(request).unwrap(),
            project_id: recovery_project_id(request),
            command_kind: command_kind(&request.command).to_owned(),
            status: CommandJournalStatus::Prepared,
            reply: None,
            occurred_at: "101".to_owned(),
        },
    )
    .unwrap();
}

fn verified_adapter(directory: &Path) -> PathBuf {
    let workflow = directory.join("workflow.json");
    write_json_atomic(
        &workflow,
        &serde_json::json!({
            "1": {"class_type": "TextEncode", "inputs": {"text": ""}},
            "2": {"class_type": "Sampler", "inputs": {"seed": 0}},
            "3": {"class_type": "VideoSave", "inputs": {"filename_prefix": ""}}
        }),
    )
    .unwrap();
    let config = directory.join("adapter.yaml");
    fs::write(
        &config,
        r#"schema_version: "1.0"
adapter: test-comfy
enabled: true
endpoint: http://127.0.0.1:8188
workflow: workflow.json
output_node: "3"
model_fingerprint: test-model
bindings:
  prompt: { node: "1", input: text }
  seed: { node: "2", input: seed }
  output_prefix: { node: "3", input: filename_prefix }
profiles:
  audition: {}
  final: {}
verified_operations: [t2v]
"#,
    )
    .unwrap();
    config
}

fn approved_project(runtime: &mut WorkerRuntime) {
    assert!(
        runtime
            .handle(request(
                None,
                None,
                WorkerCommand::CreateProject {
                    project_id: "rain-apartment".to_owned(),
                    title: "Rain Apartment".to_owned(),
                    brief: "Brief".to_owned(),
                },
            ))
            .ok
    );
    assert!(
        runtime
            .handle(request(
                Some("rain-apartment"),
                Some(1),
                WorkerCommand::ApplyScript {
                    bundle_json: BUNDLE.to_owned(),
                },
            ))
            .ok
    );
    assert!(
        runtime
            .handle(request(
                Some("rain-apartment"),
                Some(2),
                WorkerCommand::ApproveScript,
            ))
            .ok
    );
}

fn seed_candidate(
    paths: &AppPaths,
    selected: bool,
    stage: ShotStage,
    media_path: PathBuf,
) -> String {
    let store = ProjectStore::open(&paths.projects_dir, "rain-apartment").unwrap();
    let take_id = "TAKE-00000000000000000000000000".to_owned();
    let mut state = store.read_state().unwrap();
    let take = TakeMetadata {
        take_id: take_id.clone(),
        shot_id: "S01".to_owned(),
        job_id: "JOB-00000000000000000000000000".to_owned(),
        profile: "audition".to_owned(),
        status: "candidate".to_owned(),
        media_path,
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
        hard_checks: vec!["MEDIA_PROBE_OK".to_owned()],
        warnings: Vec::new(),
        stale: false,
    };
    state.takes.insert(take_id.clone(), take);
    let shot = state.shots.get_mut("S01").unwrap();
    shot.stage = stage;
    shot.take_ids.push(take_id.clone());
    shot.selected_candidate_take_id = selected.then(|| take_id.clone());
    state.bump_revision("seed".to_owned()).unwrap();
    store.save_state(&state, 3).unwrap();
    take_id
}

fn seed_candidate_approval(paths: &AppPaths, shot_id: &str) {
    let take_id = seed_candidate(
        paths,
        false,
        ShotStage::CandidatesReady,
        PathBuf::from(format!("raw/{shot_id}/candidate.mp4")),
    );
    let store = ProjectStore::open(&paths.projects_dir, "rain-apartment").unwrap();
    let mut state = store.read_state().unwrap();
    state.pending_approvals.push(Approval {
        approval_id: "APR-CANDIDATE".to_owned(),
        kind: ApprovalKind::CandidateSelection,
        subject_id: None,
        shot_id: Some(shot_id.to_owned()),
        take_ids: vec![take_id],
        blocking: true,
        description: "Select a candidate".to_owned(),
        created_at: "seed".to_owned(),
    });
    state.bump_revision("seed".to_owned()).unwrap();
    store.save_state(&state, 4).unwrap();
}

fn seed_full_audition_budget(paths: &AppPaths) {
    let first_take_id = seed_candidate(
        paths,
        false,
        ShotStage::CandidatesReady,
        PathBuf::from("raw/S01/candidate-0.mp4"),
    );
    let store = ProjectStore::open(&paths.projects_dir, "rain-apartment").unwrap();
    let mut state = store.read_state().unwrap();
    let template = state.takes[&first_take_id].clone();
    for index in 1..3 {
        let take_id = format!("TAKE-{index:026}");
        let mut take = template.clone();
        take.take_id.clone_from(&take_id);
        take.media_path = PathBuf::from(format!("raw/S01/candidate-{index}.mp4"));
        state.takes.insert(take_id.clone(), take);
        state.shots.get_mut("S01").unwrap().take_ids.push(take_id);
    }
    state.bump_revision("seed-budget".to_owned()).unwrap();
    store.save_state(&state, 4).unwrap();
}

#[test]
fn audition_command_persists_job_and_gpu_queue_entry() {
    let directory = tempfile::tempdir().unwrap();
    let paths = AppPaths::resolve(Some(directory.path().join("data")), None);
    let adapter = verified_adapter(directory.path());
    let mut runtime = WorkerRuntime::open_with_adapter(paths.clone(), Some(adapter)).unwrap();
    approved_project(&mut runtime);

    let reply = runtime.handle(request(
        Some("rain-apartment"),
        Some(3),
        WorkerCommand::AuditionShot {
            shot_id: "S01".to_owned(),
        },
    ));

    assert!(reply.ok, "{reply:?}");
    assert_eq!(reply.revision, Some(4));
    let snapshot = reply.snapshot.unwrap();
    assert_eq!(snapshot.shots[0].stage, "queued");
    assert_eq!(snapshot.queue.jobs.len(), 1);
    let job_id = &snapshot.queue.jobs[0].job_id;
    let store = ProjectStore::open(&paths.projects_dir, "rain-apartment").unwrap();
    let job = store.read_job(job_id).unwrap();
    assert_eq!(job.shot_id, "S01");
    assert_eq!(job.profile, "audition");
    assert_eq!(job.state, JobState::Queued);
    assert!(job.attempts.is_empty());
}

#[test]
fn queue_snapshot_is_rebuilt_from_project_job_source_of_truth() {
    let directory = tempfile::tempdir().unwrap();
    let paths = AppPaths::resolve(Some(directory.path().join("data")), None);
    let adapter = verified_adapter(directory.path());
    let mut runtime =
        WorkerRuntime::open_with_adapter(paths.clone(), Some(adapter.clone())).unwrap();
    approved_project(&mut runtime);
    assert!(
        runtime
            .handle(request(
                Some("rain-apartment"),
                Some(3),
                WorkerCommand::RenderShot {
                    shot_id: "S01".to_owned(),
                },
            ))
            .ok
    );
    write_json_atomic(&paths.queue_file(), &QueueState::default()).unwrap();
    drop(runtime);

    let recovered = WorkerRuntime::open_with_adapter(paths, Some(adapter)).unwrap();

    assert_eq!(recovered.queue.pending.len(), 1);
    assert_eq!(recovered.queue.pending[0].project_id, "rain-apartment");
}

#[test]
fn generation_without_adapter_does_not_mutate_project_or_queue() {
    let (_directory, mut runtime) = runtime();
    approved_project(&mut runtime);

    let reply = runtime.handle(request(
        Some("rain-apartment"),
        Some(3),
        WorkerCommand::AuditionShot {
            shot_id: "S01".to_owned(),
        },
    ));

    assert!(!reply.ok);
    assert_eq!(reply.error.unwrap().code, "ADAPTER_CONFIG_MISSING");
    assert_eq!(runtime.project_revision(Some("rain-apartment")), Some(3));
    assert!(runtime.queue.pending.is_empty());
}

#[test]
fn audition_limit_is_enforced_and_reported_in_snapshot() {
    let directory = tempfile::tempdir().unwrap();
    let paths = AppPaths::resolve(Some(directory.path().join("data")), None);
    let adapter = verified_adapter(directory.path());
    let mut runtime = WorkerRuntime::open_with_adapter(paths.clone(), Some(adapter)).unwrap();
    approved_project(&mut runtime);
    seed_full_audition_budget(&paths);

    let blocked = runtime.handle(request(
        Some("rain-apartment"),
        Some(5),
        WorkerCommand::AuditionShot {
            shot_id: "S01".to_owned(),
        },
    ));

    assert!(!blocked.ok);
    assert_eq!(blocked.error.unwrap().code, "AUDITION_LIMIT_REACHED");
    let snapshot = runtime.handle(request(
        Some("rain-apartment"),
        None,
        WorkerCommand::Snapshot,
    ));
    let budget = snapshot.snapshot.unwrap().budget;
    assert_eq!(budget.audition_takes_used, 3);
    assert_eq!(budget.audition_takes_limit, 6);
}

#[test]
fn retry_uses_audition_without_selection_and_final_with_selection() {
    for (selected, expected_profile) in [(false, "audition"), (true, "final")] {
        let directory = tempfile::tempdir().unwrap();
        let paths = AppPaths::resolve(Some(directory.path().join("data")), None);
        let adapter = verified_adapter(directory.path());
        let mut runtime = WorkerRuntime::open_with_adapter(paths.clone(), Some(adapter)).unwrap();
        approved_project(&mut runtime);
        let take_id = seed_candidate(
            &paths,
            selected,
            ShotStage::Failed,
            PathBuf::from("raw/S01/candidate.mp4"),
        );

        let reply = runtime.handle(request(
            Some("rain-apartment"),
            Some(4),
            WorkerCommand::RetryShot {
                shot_id: "S01".to_owned(),
            },
        ));

        assert!(reply.ok, "{reply:?}");
        let job_id = &reply.snapshot.unwrap().queue.jobs[0].job_id;
        let store = ProjectStore::open(&paths.projects_dir, "rain-apartment").unwrap();
        let job = store.read_job(job_id).unwrap();
        assert_eq!(job.profile, expected_profile);
        if selected {
            assert_eq!(job.seed, 7);
            assert_eq!(job.parent_take_id.as_deref(), Some(take_id.as_str()));
            assert_eq!(job.promotion_strategy, Some(PromotionStrategy::SeedReplay));
        } else {
            assert!(job.parent_take_id.is_none());
            assert!(job.promotion_strategy.is_none());
        }
    }
}

#[test]
fn take_approval_error_has_stable_worker_code() {
    let directory = tempfile::tempdir().unwrap();
    let paths = AppPaths::resolve(Some(directory.path().join("data")), None);
    let mut runtime = WorkerRuntime::open(paths.clone()).unwrap();
    approved_project(&mut runtime);
    let take_id = seed_candidate(
        &paths,
        false,
        ShotStage::CandidatesReady,
        PathBuf::from("raw/S01/candidate.mp4"),
    );

    let reply = runtime.handle(request(
        Some("rain-apartment"),
        Some(4),
        WorkerCommand::ApproveTake {
            shot_id: "S01".to_owned(),
            take_id,
        },
    ));

    assert!(!reply.ok);
    assert_eq!(reply.error.unwrap().code, "TAKE_NOT_SELECTED");
}

#[test]
fn preview_returns_existing_media_and_rejects_unsafe_paths() {
    let directory = tempfile::tempdir().unwrap();
    let paths = AppPaths::resolve(Some(directory.path().join("data")), None);
    let mut runtime = WorkerRuntime::open(paths.clone()).unwrap();
    approved_project(&mut runtime);
    let media_path = PathBuf::from("raw/S01/candidate.mp4");
    let take_id = seed_candidate(
        &paths,
        false,
        ShotStage::CandidatesReady,
        media_path.clone(),
    );
    let store = ProjectStore::open(&paths.projects_dir, "rain-apartment").unwrap();
    fs::create_dir_all(store.root().join("raw/S01")).unwrap();
    fs::write(store.root().join(&media_path), b"video").unwrap();

    let reply = runtime.handle(request(
        Some("rain-apartment"),
        None,
        WorkerCommand::PreviewTake {
            take_id: take_id.clone(),
        },
    ));
    assert!(reply.ok, "{reply:?}");
    assert_eq!(reply.artifact_path, Some(store.root().join(media_path)));

    let mut state = store.read_state().unwrap();
    state.takes.get_mut(&take_id).unwrap().media_path = PathBuf::from("../outside.mp4");
    state.bump_revision("unsafe".to_owned()).unwrap();
    store.save_state(&state, 4).unwrap();
    let reply = runtime.handle(request(
        Some("rain-apartment"),
        None,
        WorkerCommand::PreviewTake { take_id },
    ));
    assert!(!reply.ok);
    assert_eq!(reply.error.unwrap().code, "ARTIFACT_PATH_INVALID");
}

#[test]
fn completed_candidate_creates_blocking_selection_approval() {
    let directory = tempfile::tempdir().unwrap();
    let paths = AppPaths::resolve(Some(directory.path().join("data")), None);
    let mut runtime = WorkerRuntime::open(paths.clone()).unwrap();
    approved_project(&mut runtime);
    let take_id = seed_candidate(
        &paths,
        false,
        ShotStage::CandidatesReady,
        PathBuf::from("raw/S01/candidate.mp4"),
    );
    let store = ProjectStore::open(&paths.projects_dir, "rain-apartment").unwrap();
    let mut state = store.read_state().unwrap();
    let take = state.takes[&take_id].clone();

    register_candidate(&mut state, &take, "S01", "completed");

    let approval = state
        .pending_approvals
        .iter()
        .find(|approval| approval.kind == ApprovalKind::CandidateSelection)
        .unwrap();
    assert!(approval.blocking);
    assert_eq!(approval.shot_id.as_deref(), Some("S01"));
    assert_eq!(approval.take_ids, vec![take_id]);
}

#[test]
fn candidate_approval_allows_more_auditions_and_does_not_freeze_other_shots() {
    for command in [
        WorkerCommand::AuditionShot {
            shot_id: "S01".to_owned(),
        },
        WorkerCommand::RenderShot {
            shot_id: "S02".to_owned(),
        },
    ] {
        let directory = tempfile::tempdir().unwrap();
        let paths = AppPaths::resolve(Some(directory.path().join("data")), None);
        let adapter = verified_adapter(directory.path());
        let mut runtime = WorkerRuntime::open_with_adapter(paths.clone(), Some(adapter)).unwrap();
        approved_project(&mut runtime);
        seed_candidate_approval(&paths, "S01");

        let reply = runtime.handle(request(Some("rain-apartment"), Some(5), command));

        assert!(reply.ok, "{reply:?}");
    }

    let directory = tempfile::tempdir().unwrap();
    let paths = AppPaths::resolve(Some(directory.path().join("data")), None);
    let adapter = verified_adapter(directory.path());
    let mut runtime = WorkerRuntime::open_with_adapter(paths.clone(), Some(adapter)).unwrap();
    approved_project(&mut runtime);
    seed_candidate_approval(&paths, "S01");
    let blocked = runtime.handle(request(
        Some("rain-apartment"),
        Some(5),
        WorkerCommand::RenderShot {
            shot_id: "S01".to_owned(),
        },
    ));
    assert!(!blocked.ok);
    assert_eq!(blocked.error.unwrap().code, "APPROVAL_REQUIRED");
}

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

    runtime
        .apply_executor_event(ExecutorEvent::Submitted {
            job_id: job_id.clone(),
            request_id: request_id.clone(),
            backend_job_id: crate::adapter::BackendJobId("backend-1".to_owned()),
        })
        .unwrap();
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
    assert_eq!(state.shots["S01"].stage, ShotStage::CandidatesReady);
    assert!(state.shots["S01"].active_job_id.is_none());
    assert!(state.pending_approvals.iter().any(|approval| {
        approval.kind == ApprovalKind::CandidateSelection && approval.take_ids.contains(take_id)
    }));
    assert!(runtime.queue.running.is_none());
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
fn startup_recovers_completed_job_before_project_state_commit() {
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
    assert!(state.shots["S01"].active_job_id.is_none());
    assert!(state.takes.contains_key(&take.take_id));
    assert!(state.pending_approvals.iter().any(|approval| {
        approval.kind == ApprovalKind::CandidateSelection
            && approval.take_ids.contains(&take.take_id)
    }));
    assert!(recovered.queue.running.is_none());
    assert!(recovered.queue.pending.is_empty());
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

#[test]
fn pending_job_can_be_cancelled_and_shot_returns_to_pending() {
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

    let reply = runtime.handle(request(
        Some("rain-apartment"),
        Some(4),
        WorkerCommand::CancelJob {
            job_id: job_id.clone(),
        },
    ));

    assert!(reply.ok, "{reply:?}");
    let snapshot = reply.snapshot.unwrap();
    assert!(snapshot.queue.jobs.is_empty());
    assert_eq!(snapshot.shots[0].stage, "pending");
    let store = ProjectStore::open(&paths.projects_dir, "rain-apartment").unwrap();
    assert_eq!(store.read_job(&job_id).unwrap().state, JobState::Cancelled);
}

#[test]
fn running_job_is_not_marked_cancelled_without_backend_interrupt() {
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
    let _request = runtime.next_executor_request().unwrap().unwrap();

    let reply = runtime.handle(request(
        Some("rain-apartment"),
        Some(5),
        WorkerCommand::CancelJob {
            job_id: job_id.clone(),
        },
    ));

    assert!(!reply.ok);
    assert_eq!(reply.error.unwrap().code, "JOB_ALREADY_RUNNING");
    let store = ProjectStore::open(&paths.projects_dir, "rain-apartment").unwrap();
    assert_eq!(store.read_job(&job_id).unwrap().state, JobState::Active);
}
