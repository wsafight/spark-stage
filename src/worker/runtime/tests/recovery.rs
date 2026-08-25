use super::*;

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
