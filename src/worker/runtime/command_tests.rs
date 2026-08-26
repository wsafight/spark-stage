use super::*;

fn request(project_id: &str, command: WorkerCommand) -> ClientRequest {
    ClientRequest {
        protocol_version: IPC_PROTOCOL_VERSION.to_owned(),
        command_id: Ulid::new().to_string(),
        expected_revision: None,
        project_id: Some(project_id.to_owned()),
        command,
    }
}

fn runtime_with_project() -> (tempfile::TempDir, WorkerRuntime, ProjectStore) {
    let directory = tempfile::tempdir().unwrap();
    let paths = AppPaths::resolve(Some(directory.path().join("data")), None);
    let mut runtime = WorkerRuntime::open(paths.clone()).unwrap();
    let create = ClientRequest {
        protocol_version: IPC_PROTOCOL_VERSION.to_owned(),
        command_id: Ulid::new().to_string(),
        expected_revision: None,
        project_id: None,
        command: WorkerCommand::CreateProject {
            project_id: "demo".to_owned(),
            title: "Demo".to_owned(),
            brief: "Brief".to_owned(),
        },
    };
    assert!(runtime.handle(create).ok);
    let store = ProjectStore::open(&paths.projects_dir, "demo").unwrap();
    (directory, runtime, store)
}

#[test]
fn logs_and_worker_probe_are_read_only_artifacts() {
    let (_directory, mut runtime, store) = runtime_with_project();
    let command_count = runtime.commands.len();

    let logs = runtime.handle(request("demo", WorkerCommand::OpenLogs));
    assert!(logs.ok, "{logs:?}");
    assert_eq!(logs.revision, Some(1));
    assert_eq!(logs.artifact_path, Some(store.root().join("logs")));

    let probe = runtime.handle(request(
        "demo",
        WorkerCommand::RetryProbe {
            probe_id: "worker".to_owned(),
        },
    ));
    assert!(probe.ok, "{probe:?}");
    assert_eq!(probe.snapshot.unwrap().diagnostics[0].probe_id, "worker");
    assert_eq!(runtime.commands.len(), command_count);
}

#[test]
fn unknown_probe_has_stable_error_code() {
    let (_directory, mut runtime, _store) = runtime_with_project();

    let reply = runtime.handle(request(
        "demo",
        WorkerCommand::RetryProbe {
            probe_id: "missing".to_owned(),
        },
    ));

    assert!(!reply.ok);
    assert_eq!(reply.error.unwrap().code, "PROBE_NOT_FOUND");
}

#[test]
fn open_build_validates_record_path_and_artifact_existence() {
    let (_directory, mut runtime, store) = runtime_with_project();
    let mut state = store.read_state().unwrap();
    state.builds.insert(
        "BLD-test".to_owned(),
        crate::domain::BuildRecord {
            build_id: "BLD-test".to_owned(),
            kind: "draft".to_owned(),
            status: "needs_review".to_owned(),
            recipe: "builds/BLD-test/recipe.json".to_owned(),
            command_id: "build-command".to_owned(),
            output_path: Some(PathBuf::from("builds/BLD-test/output.mp4")),
            warnings: Vec::new(),
            stale: false,
        },
    );
    state.bump_revision("101".to_owned()).unwrap();
    store.save_state(&state, 1).unwrap();

    let missing = runtime.handle(request(
        "demo",
        WorkerCommand::OpenBuild {
            build_id: "BLD-test".to_owned(),
        },
    ));
    assert_eq!(missing.error.unwrap().code, "ARTIFACT_NOT_FOUND");

    let output = store.root().join("builds/BLD-test/output.mp4");
    fs::create_dir_all(output.parent().unwrap()).unwrap();
    fs::write(&output, b"video").unwrap();
    let opened = runtime.handle(request(
        "demo",
        WorkerCommand::OpenBuild {
            build_id: "BLD-test".to_owned(),
        },
    ));
    assert!(opened.ok, "{opened:?}");
    assert_eq!(opened.artifact_path, Some(output));

    let mut state = store.read_state().unwrap();
    state.builds.get_mut("BLD-test").unwrap().output_path = Some(PathBuf::from("../outside.mp4"));
    state.bump_revision("102".to_owned()).unwrap();
    store.save_state(&state, 2).unwrap();
    let unsafe_path = runtime.handle(request(
        "demo",
        WorkerCommand::OpenBuild {
            build_id: "BLD-test".to_owned(),
        },
    ));
    assert_eq!(unsafe_path.error.unwrap().code, "ARTIFACT_PATH_INVALID");
}

#[test]
fn invalid_build_kind_is_rejected_before_media_execution() {
    let (_directory, mut runtime, _store) = runtime_with_project();
    let mut build = request(
        "demo",
        WorkerCommand::Build {
            kind: "unknown".to_owned(),
            shot_ids: Vec::new(),
        },
    );
    build.expected_revision = Some(1);

    let reply = runtime.handle(build);

    assert!(!reply.ok);
    assert_eq!(reply.error.unwrap().code, "BUILD_KIND_INVALID");
}

#[test]
fn scoped_final_build_is_rejected_before_media_execution() {
    let (_directory, mut runtime, _store) = runtime_with_project();
    let mut build = request(
        "demo",
        WorkerCommand::Build {
            kind: "final".to_owned(),
            shot_ids: vec!["S01".to_owned()],
        },
    );
    build.expected_revision = Some(1);

    let reply = runtime.handle(build);

    assert!(!reply.ok);
    assert_eq!(reply.error.unwrap().code, "BUILD_SCOPE_INVALID");
}

#[test]
fn startup_resumes_running_build_recipe_and_persists_failure() {
    let (_directory, runtime, store) = runtime_with_project();
    let paths = runtime.paths.clone();
    let build_id = "BLD-recovery";
    let recipe = crate::build::BuildRecipe {
        schema_version: crate::build::BUILD_RECIPE_SCHEMA_VERSION.to_owned(),
        build_id: build_id.to_owned(),
        project_id: "demo".to_owned(),
        contract_id: None,
        contract_hash: "contract".to_owned(),
        source_revision: 1,
        kind: crate::build::BuildKind::Draft,
        width: 960,
        height: 544,
        fps: 24,
        expected_duration_seconds: 5,
        inputs: vec![crate::build::BuildInput {
            shot_id: "S01".to_owned(),
            take_id: "TAKE-missing".to_owned(),
            media_path: PathBuf::from("raw/S01/missing.mp4"),
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
        output_path: PathBuf::from("builds/BLD-recovery/output.mp4"),
        delivery_path: PathBuf::from("review/draft-cut.mp4"),
    };
    let recipe_path = PathBuf::from("builds/BLD-recovery/recipe.json");
    write_json_atomic(&store.root().join(&recipe_path), &recipe).unwrap();
    store
        .start_build(
            crate::domain::BuildRecord {
                build_id: build_id.to_owned(),
                kind: "draft".to_owned(),
                status: "queued".to_owned(),
                recipe: recipe_path.to_string_lossy().into_owned(),
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
    let queued_revision = store.read_state().unwrap().revision;
    store
        .mark_build_running(build_id, queued_revision, "build", "102")
        .unwrap();
    drop(runtime);

    let mut recovered = WorkerRuntime::open(paths).unwrap();
    for _ in 0..100 {
        recovered.poll_build_events().unwrap();
        if store.read_state().unwrap().builds[build_id].status == "failed" {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    let state = store.read_state().unwrap();
    assert_eq!(state.builds[build_id].status, "failed");
    assert!(!state.builds[build_id].warnings.is_empty());
}

#[test]
fn startup_marks_missing_build_recipe_failed_without_blocking_worker() {
    let (_directory, runtime, store) = runtime_with_project();
    let paths = runtime.paths.clone();
    let build_id = "BLD-missing-recipe";
    store
        .start_build(
            crate::domain::BuildRecord {
                build_id: build_id.to_owned(),
                kind: "draft".to_owned(),
                status: "queued".to_owned(),
                recipe: "builds/BLD-missing-recipe/recipe.json".to_owned(),
                command_id: "original-build-command".to_owned(),
                output_path: None,
                warnings: Vec::new(),
                stale: false,
            },
            1,
            "original-build-command",
            "101",
        )
        .unwrap();
    drop(runtime);

    let _recovered = WorkerRuntime::open(paths).unwrap();

    let state = store.read_state().unwrap();
    assert_eq!(state.builds[build_id].status, "failed");
    assert_eq!(
        state.last_command_id.as_deref(),
        Some("original-build-command")
    );
    assert!(state.builds[build_id].warnings[0].contains("build recovery failed"));
}
