use super::*;

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
fn running_job_is_not_cancelled_before_backend_submission() {
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
    assert_eq!(reply.error.unwrap().code, "JOB_CANCEL_NOT_READY");
    let store = ProjectStore::open(&paths.projects_dir, "rain-apartment").unwrap();
    assert_eq!(store.read_job(&job_id).unwrap().state, JobState::Active);
}

#[test]
fn running_job_requires_explicit_global_interrupt_opt_in() {
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
    mark_backend_submitted(&paths, &job_id);

    let reply = runtime.handle(request(
        Some("rain-apartment"),
        Some(5),
        WorkerCommand::CancelJob {
            job_id: job_id.clone(),
        },
    ));

    assert!(!reply.ok);
    assert_eq!(reply.error.unwrap().code, "JOB_CANCEL_UNSUPPORTED");
    let store = ProjectStore::open(&paths.projects_dir, "rain-apartment").unwrap();
    assert_eq!(store.read_job(&job_id).unwrap().state, JobState::Active);
}

#[test]
fn running_job_is_cancelled_only_after_backend_interrupt_succeeds() {
    let directory = tempfile::tempdir().unwrap();
    let paths = AppPaths::resolve(Some(directory.path().join("data")), None);
    let (adapter, interrupt_request) = interrupting_adapter(directory.path());
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
    mark_backend_submitted(&paths, &job_id);

    let reply = runtime.handle(request(
        Some("rain-apartment"),
        Some(5),
        WorkerCommand::CancelJob {
            job_id: job_id.clone(),
        },
    ));

    assert!(reply.ok, "{reply:?}");
    let request = interrupt_request
        .recv_timeout(Duration::from_secs(1))
        .unwrap();
    assert!(request.starts_with("POST /interrupt HTTP/1.1"));
    let snapshot = reply.snapshot.unwrap();
    assert!(snapshot.queue.jobs.is_empty());
    assert_eq!(snapshot.shots[0].stage, "pending");
    let store = ProjectStore::open(&paths.projects_dir, "rain-apartment").unwrap();
    let job = store.read_job(&job_id).unwrap();
    assert_eq!(job.state, JobState::Cancelled);
    assert_eq!(job.attempts[0].state, AttemptState::Cancelled);
    assert_eq!(
        job.attempts[0].error_code.as_deref(),
        Some("USER_CANCELLED")
    );
}
