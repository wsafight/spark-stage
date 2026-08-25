use super::*;

#[test]
fn project_list_is_sorted_and_surfaces_pause_state() {
    let (_directory, mut runtime) = runtime();
    for (project_id, title) in [("zeta", "Zeta"), ("alpha", "Alpha")] {
        assert!(
            runtime
                .handle(request(
                    None,
                    None,
                    WorkerCommand::CreateProject {
                        project_id: project_id.to_owned(),
                        title: title.to_owned(),
                        brief: "Brief".to_owned(),
                    },
                ))
                .ok
        );
    }
    let paused = runtime.handle(request(Some("zeta"), Some(1), WorkerCommand::PauseProject));
    assert!(paused.ok, "{paused:?}");
    assert!(paused.snapshot.unwrap().project.paused);
    let command_count = runtime.commands.len();

    let listed = runtime.handle(request(None, None, WorkerCommand::ListProjects));

    assert!(listed.ok, "{listed:?}");
    let Some(WorkerPayload::ProjectList { projects }) = listed.payload else {
        panic!("expected project list payload");
    };
    assert_eq!(
        projects
            .iter()
            .map(|project| project.id.as_str())
            .collect::<Vec<_>>(),
        ["alpha", "zeta"]
    );
    assert!(!projects[0].paused);
    assert!(projects[1].paused);
    assert_eq!(projects[1].revision, Some(2));
    assert_eq!(runtime.commands.len(), command_count);
}

#[test]
fn project_pause_requires_current_revision() {
    let (_directory, mut runtime) = runtime();
    assert!(
        runtime
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

    let stale = runtime.handle(request(Some("demo"), Some(99), WorkerCommand::PauseProject));

    assert!(!stale.ok);
    assert_eq!(stale.error.unwrap().code, "REVISION_CONFLICT");
    assert!(
        !ProjectStore::open(&runtime.paths.projects_dir, "demo")
            .unwrap()
            .read_state()
            .unwrap()
            .paused
    );
}

#[test]
fn paused_project_keeps_pending_job_until_resumed() {
    let directory = tempfile::tempdir().unwrap();
    let paths = AppPaths::resolve(Some(directory.path().join("data")), None);
    let adapter = verified_adapter(directory.path());
    let mut runtime = WorkerRuntime::open_with_adapter(paths, Some(adapter)).unwrap();
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
    let queued = runtime.handle(request(
        Some("rain-apartment"),
        Some(3),
        WorkerCommand::AuditionShot {
            shot_id: "S01".to_owned(),
        },
    ));
    assert!(queued.ok, "{queued:?}");
    let paused = runtime.handle(request(
        Some("rain-apartment"),
        queued.revision,
        WorkerCommand::PauseProject,
    ));
    assert!(paused.ok, "{paused:?}");

    assert!(runtime.next_executor_request().unwrap().is_none());
    assert_eq!(runtime.queue.pending.len(), 1);

    let resumed = runtime.handle(request(
        Some("rain-apartment"),
        paused.revision,
        WorkerCommand::ResumeProject,
    ));
    assert!(resumed.ok, "{resumed:?}");
    assert!(matches!(
        runtime.next_executor_request().unwrap(),
        Some(ExecutorRequest::Prepare(_))
    ));
}
