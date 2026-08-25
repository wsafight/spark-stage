use super::*;

#[test]
fn decision_history_is_newest_first_and_respects_limit() {
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
    let paused = runtime.handle(request(Some("demo"), Some(1), WorkerCommand::PauseProject));
    assert!(paused.ok, "{paused:?}");
    let resumed = runtime.handle(request(
        Some("demo"),
        paused.revision,
        WorkerCommand::ResumeProject,
    ));
    assert!(resumed.ok, "{resumed:?}");

    let history = runtime.handle(request(
        Some("demo"),
        None,
        WorkerCommand::DecisionHistory { limit: 1 },
    ));

    assert!(history.ok, "{history:?}");
    assert_eq!(history.revision, resumed.revision);
    let Some(WorkerPayload::DecisionHistory { decisions }) = history.payload else {
        panic!("expected decision history");
    };
    assert_eq!(decisions.len(), 1);
    assert_eq!(decisions[0].kind, "project_resumed");
    assert_eq!(decisions[0].subject_id, "demo");
}

#[test]
fn decision_history_rejects_invalid_limit_without_mutation() {
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

    let invalid = runtime.handle(request(
        Some("demo"),
        None,
        WorkerCommand::DecisionHistory { limit: 0 },
    ));

    assert!(!invalid.ok);
    assert_eq!(invalid.error.unwrap().code, "HISTORY_LIMIT_INVALID");
    assert_eq!(runtime.project_revision(Some("demo")), Some(1));
}
