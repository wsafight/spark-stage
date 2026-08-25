use super::*;

#[test]
fn schema_write_is_atomic_and_parseable() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("nested/script-bundle.schema.json");

    assert_eq!(write_schema(Some(&path)).unwrap(), ExitCode::SUCCESS);
    let value: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
    assert_eq!(value["title"], "ScriptBundle");
}

#[test]
fn tui_arguments_parse_without_starting_terminal() {
    let cli = Cli::try_parse_from([
        "sparkstage",
        "tui",
        "--socket",
        "/tmp/sparkstage-test.sock",
        "--project",
        "rain-apartment",
        "--refresh-ms",
        "250",
    ])
    .unwrap();

    let Command::Tui(args) = cli.command else {
        panic!("expected tui command");
    };
    assert_eq!(
        args.socket,
        Some(PathBuf::from("/tmp/sparkstage-test.sock"))
    );
    assert_eq!(args.project.as_deref(), Some("rain-apartment"));
    assert_eq!(args.refresh_ms, 250);
}

#[test]
fn shot_decision_commands_parse_stable_ids() {
    let cli = Cli::try_parse_from([
        "sparkstage",
        "shots",
        "select",
        "--project",
        "rain-apartment",
        "--shot",
        "S01",
        "--take",
        "TAKE-01ARZ3NDEKTSV4RRFFQ69G5FAV",
        "--json",
    ])
    .unwrap();

    let Command::Shots(ShotsArgs {
        command:
            ShotsCommand::Select {
                project,
                shot,
                take,
                json,
                ..
            },
    }) = cli.command
    else {
        panic!("expected shots select command");
    };
    assert_eq!(project, "rain-apartment");
    assert_eq!(shot, "S01");
    assert_eq!(take, "TAKE-01ARZ3NDEKTSV4RRFFQ69G5FAV");
    assert!(json);
}

#[test]
fn edit_commands_parse_build_kind_and_build_id() {
    let cli = Cli::try_parse_from([
        "sparkstage",
        "edit",
        "build",
        "--project",
        "rain-apartment",
        "--kind",
        "draft",
    ])
    .unwrap();
    let Command::Edit(EditArgs {
        command:
            EditCommand::Build {
                project,
                kind,
                shots,
                ..
            },
    }) = cli.command
    else {
        panic!("expected edit build command");
    };
    assert_eq!(project, "rain-apartment");
    assert_eq!(kind, "draft");
    assert_eq!(shots, None);

    let cli = Cli::try_parse_from([
        "sparkstage",
        "edit",
        "open",
        "--project",
        "rain-apartment",
        "--build",
        "BLD-01ARZ3NDEKTSV4RRFFQ69G5FAV",
    ])
    .unwrap();
    assert!(matches!(
        cli.command,
        Command::Edit(EditArgs {
            command: EditCommand::Open { .. }
        })
    ));
}

#[test]
fn shot_selection_expands_ranges_and_rejects_ambiguous_values() {
    assert_eq!(
        expand_shot_selection("S04-S07,S10").unwrap(),
        ["S04", "S05", "S06", "S07", "S10"]
    );
    assert!(expand_shot_selection("S07-S04").is_err());
    assert!(expand_shot_selection("S01-T03").is_err());
    assert!(expand_shot_selection("S01,,S03").is_err());
}

#[test]
fn shot_selection_trims_items_and_preserves_range_padding() {
    assert_eq!(
        expand_shot_selection(" S1-S003, CUT09 ").unwrap(),
        ["S001", "S002", "S003", "CUT09"]
    );
}

#[test]
fn shot_selection_rejects_prefixless_overflow_and_malformed_ranges() {
    for selection in ["1-3", "S01-S02-S03", "S4294967296-S4294967297", "S01-"] {
        assert!(
            expand_shot_selection(selection).is_err(),
            "{selection} should be rejected"
        );
    }
}

#[test]
fn shot_selection_enforces_expansion_limit() {
    let error = expand_shot_selection("S0000-S1000").unwrap_err();
    assert!(error.contains("beyond 1000 items"));
}

#[test]
fn tui_refresh_interval_enforces_cli_bounds() {
    for refresh_ms in ["99", "60001"] {
        assert!(
            Cli::try_parse_from(["sparkstage", "tui", "--refresh-ms", refresh_ms]).is_err(),
            "{refresh_ms} should be outside the supported range"
        );
    }
    assert!(Cli::try_parse_from(["sparkstage", "tui", "--refresh-ms", "100"]).is_ok());
    assert!(Cli::try_parse_from(["sparkstage", "tui", "--refresh-ms", "60000"]).is_ok());
}

#[test]
fn scoped_final_build_is_rejected_before_worker_connection() {
    let cli = Cli::try_parse_from([
        "sparkstage",
        "edit",
        "build",
        "--project",
        "rain-apartment",
        "--kind",
        "final",
        "--shots",
        "S01-S03",
    ])
    .unwrap();
    let Command::Edit(args) = cli.command else {
        panic!("expected edit command");
    };

    let error = execute_edit(args).unwrap_err();

    assert!(matches!(
        error,
        CliError::InvalidInput(message) if message == "--shots is only valid with --kind draft"
    ));
}

#[test]
fn control_commands_parse_stable_resource_ids() {
    let queue = Cli::try_parse_from([
        "sparkstage",
        "queue",
        "cancel",
        "--project",
        "rain-apartment",
        "--job",
        "JOB-01ARZ3NDEKTSV4RRFFQ69G5FAV",
    ])
    .unwrap();
    assert!(matches!(queue.command, Command::Queue(_)));

    let approval = Cli::try_parse_from([
        "sparkstage",
        "approval",
        "approve",
        "--project",
        "rain-apartment",
        "--approval",
        "APR-01ARZ3NDEKTSV4RRFFQ69G5FAV",
    ])
    .unwrap();
    assert!(matches!(approval.command, Command::Approval(_)));

    for arguments in [
        ["diagnostics", "retry", "--probe", "worker"],
        ["logs", "open", "--project", "rain-apartment"],
    ] {
        let mut command = vec!["sparkstage"];
        command.extend(arguments);
        if command[1] == "diagnostics" {
            command.extend(["--project", "rain-apartment"]);
        }
        Cli::try_parse_from(command).unwrap();
    }
}

#[test]
fn project_management_commands_parse_without_side_effects() {
    let list = Cli::try_parse_from(["sparkstage", "project", "list", "--json"]).unwrap();
    assert!(matches!(
        list.command,
        Command::Project(ProjectArgs {
            command: ProjectSubcommand::List { json: true, .. }
        })
    ));

    for action in ["pause", "resume"] {
        let parsed = Cli::try_parse_from([
            "sparkstage",
            "project",
            action,
            "--project",
            "rain-apartment",
        ])
        .unwrap();
        assert!(matches!(parsed.command, Command::Project(_)));
    }
}

#[test]
fn project_portability_commands_parse_without_worker() {
    for arguments in [
        vec!["verify", "--project", "rain-apartment", "--json"],
        vec![
            "export",
            "--project",
            "rain-apartment",
            "--output",
            "rain.sparkstage.tar",
        ],
        vec!["verify-archive", "--archive", "rain.sparkstage.tar"],
        vec!["import", "--archive", "rain.sparkstage.tar", "--json"],
        vec!["migrate", "--project", "rain-apartment", "--apply"],
    ] {
        let mut command = vec!["sparkstage", "project"];
        command.extend(arguments);
        let parsed = Cli::try_parse_from(command).unwrap();
        assert!(matches!(parsed.command, Command::Project(_)));
    }
}

#[test]
fn project_portability_commands_execute_without_worker() {
    let directory = tempfile::tempdir().unwrap();
    let source_data = directory.path().join("source");
    let destination_data = directory.path().join("destination");
    crate::store::ProjectStore::create(
        &source_data.join("projects"),
        "portable",
        "Portable",
        "brief",
        "CMD-create",
        "100",
    )
    .unwrap();
    let archive = directory.path().join("portable.sparkstage.tar");

    for arguments in [
        vec![
            "verify".to_owned(),
            "--project".to_owned(),
            "portable".to_owned(),
            "--data-dir".to_owned(),
            source_data.display().to_string(),
        ],
        vec![
            "export".to_owned(),
            "--project".to_owned(),
            "portable".to_owned(),
            "--output".to_owned(),
            archive.display().to_string(),
            "--data-dir".to_owned(),
            source_data.display().to_string(),
        ],
        vec![
            "verify-archive".to_owned(),
            "--archive".to_owned(),
            archive.display().to_string(),
        ],
        vec![
            "migrate".to_owned(),
            "--project".to_owned(),
            "portable".to_owned(),
            "--data-dir".to_owned(),
            source_data.display().to_string(),
        ],
        vec![
            "import".to_owned(),
            "--archive".to_owned(),
            archive.display().to_string(),
            "--data-dir".to_owned(),
            destination_data.display().to_string(),
        ],
    ] {
        let mut command = vec!["sparkstage".to_owned(), "project".to_owned()];
        command.extend(arguments);
        assert_eq!(
            execute(Cli::try_parse_from(command).unwrap()).unwrap(),
            ExitCode::SUCCESS
        );
    }
    assert!(archive.is_file());
    assert!(
        destination_data
            .join("projects/portable/state.json")
            .is_file()
    );
}

#[test]
fn storage_commands_parse_project_and_plan_ids() {
    for action in ["status", "plan"] {
        let parsed = Cli::try_parse_from([
            "sparkstage",
            "storage",
            action,
            "--project",
            "rain-apartment",
            "--json",
        ])
        .unwrap();
        assert!(matches!(parsed.command, Command::Storage(_)));
    }

    for action in ["apply", "restore"] {
        let parsed = Cli::try_parse_from([
            "sparkstage",
            "storage",
            action,
            "--project",
            "rain-apartment",
            "--plan",
            "CLN-01ARZ3NDEKTSV4RRFFQ69G5FAV",
        ])
        .unwrap();
        assert!(matches!(parsed.command, Command::Storage(_)));
    }
}

#[test]
fn budget_commands_parse_contract_workflow() {
    for arguments in [
        vec!["budget", "status", "--project", "rain-apartment", "--json"],
        vec![
            "budget",
            "apply",
            "--project",
            "rain-apartment",
            "--contract",
            "budget.json",
        ],
        vec!["budget", "default", "--output", "budget.json"],
    ] {
        let mut command = vec!["sparkstage"];
        command.extend(arguments);
        let parsed = Cli::try_parse_from(command).unwrap();
        assert!(matches!(parsed.command, Command::Budget(_)));
    }
}

#[test]
fn batch_review_command_parses_file_and_approval_mode() {
    let parsed = Cli::try_parse_from([
        "sparkstage",
        "shots",
        "review",
        "--project",
        "rain-apartment",
        "--file",
        "review.json",
        "--approve",
        "--json",
    ])
    .unwrap();
    let Command::Shots(ShotsArgs {
        command:
            ShotsCommand::Review {
                project,
                file,
                approve,
                json,
                ..
            },
    }) = parsed.command
    else {
        panic!("expected shots review command");
    };
    assert_eq!(project, "rain-apartment");
    assert_eq!(file, PathBuf::from("review.json"));
    assert!(approve);
    assert!(json);
}

#[test]
fn decision_history_command_parses_bounded_limit() {
    let parsed = Cli::try_parse_from([
        "sparkstage",
        "history",
        "decisions",
        "--project",
        "rain-apartment",
        "--limit",
        "25",
    ])
    .unwrap();
    assert!(matches!(parsed.command, Command::History(_)));
    assert!(
        Cli::try_parse_from([
            "sparkstage",
            "history",
            "decisions",
            "--project",
            "rain-apartment",
            "--limit",
            "0",
        ])
        .is_err()
    );
}

#[test]
fn h3_benchmark_commands_parse_without_gpu_access() {
    for arguments in [
        vec!["init", "--adapter-config", "adapter.yaml"],
        vec![
            "record",
            "--run",
            "H3RUN-01ARZ3NDEKTSV4RRFFQ69G5FAV",
            "--sample",
            "sample.json",
        ],
        vec!["show", "--run", "H3RUN-01ARZ3NDEKTSV4RRFFQ69G5FAV"],
    ] {
        let mut command = vec!["sparkstage", "benchmark", "h3"];
        command.extend(arguments);
        let parsed = Cli::try_parse_from(command).unwrap();
        assert!(matches!(parsed.command, Command::Benchmark(_)));
    }
}

#[test]
fn adapter_scaffold_command_requires_explicit_core_bindings() {
    let parsed = Cli::try_parse_from([
        "sparkstage",
        "adapter",
        "scaffold",
        "--workflow",
        "workflow.json",
        "--output",
        "adapter.yaml",
        "--output-node",
        "120",
        "--model-fingerprint",
        "model-hash",
        "--prompt",
        "45.text",
        "--seed",
        "78.noise_seed",
        "--output-prefix",
        "120.filename_prefix",
        "--binding",
        "first_frame=90.image",
    ])
    .unwrap();
    assert!(matches!(parsed.command, Command::Adapter(_)));

    assert!(
        Cli::try_parse_from([
            "sparkstage",
            "adapter",
            "scaffold",
            "--workflow",
            "workflow.json",
            "--output",
            "adapter.yaml",
        ])
        .is_err()
    );
}

#[test]
fn worker_business_failure_returns_invalid_exit_code() {
    let reply = WorkerReply {
        protocol_version: crate::ipc::IPC_PROTOCOL_VERSION.to_owned(),
        command_id: "command".to_owned(),
        ok: false,
        revision: Some(7),
        snapshot: None,
        payload: None,
        artifact_path: None,
        message: None,
        error: Some(crate::ipc::WorkerError {
            code: "REVISION_CONFLICT".to_owned(),
            message: "refresh first".to_owned(),
            retryable: true,
            current_revision: Some(7),
        }),
    };

    assert_eq!(reply_exit_code(&reply), ExitCode::from(EXIT_INVALID));
}

#[test]
fn offline_adapter_budget_and_benchmark_commands_execute_end_to_end() {
    let directory = tempfile::tempdir().unwrap();
    let workflow = directory.path().join("workflow.json");
    fs::write(
        &workflow,
        serde_json::to_vec_pretty(&serde_json::json!({
            "45": {"class_type": "Text", "inputs": {"text": ""}},
            "78": {"class_type": "Seed", "inputs": {"noise_seed": 0}},
            "90": {"class_type": "Size", "inputs": {"width": 960}},
            "120": {"class_type": "Output", "inputs": {"filename_prefix": "out"}}
        }))
        .unwrap(),
    )
    .unwrap();
    let adapter = directory.path().join("adapter.yaml");
    let data_dir = directory.path().join("data");
    let budget = directory.path().join("contracts/budget.json");

    let adapter_command = vec![
        "sparkstage".to_owned(),
        "adapter".to_owned(),
        "scaffold".to_owned(),
        "--workflow".to_owned(),
        workflow.display().to_string(),
        "--output".to_owned(),
        adapter.display().to_string(),
        "--output-node".to_owned(),
        "120".to_owned(),
        "--model-fingerprint".to_owned(),
        "model-hash".to_owned(),
        "--prompt".to_owned(),
        "45.text".to_owned(),
        "--seed".to_owned(),
        "78.noise_seed".to_owned(),
        "--output-prefix".to_owned(),
        "120.filename_prefix".to_owned(),
        "--binding".to_owned(),
        "width=90.width".to_owned(),
    ];
    assert_eq!(
        execute(Cli::try_parse_from(adapter_command.clone()).unwrap()).unwrap(),
        ExitCode::SUCCESS
    );
    let config: crate::adapter::ComfyAdapterConfig =
        serde_yaml_ng::from_str(&fs::read_to_string(&adapter).unwrap()).unwrap();
    assert!(!config.enabled);
    assert!(config.verified_operations.is_empty());
    assert!(matches!(
        execute(Cli::try_parse_from(adapter_command).unwrap()),
        Err(CliError::InvalidInput(message)) if message.contains("already exists")
    ));

    assert_eq!(
        execute(
            Cli::try_parse_from([
                "sparkstage",
                "budget",
                "default",
                "--output",
                &budget.display().to_string(),
            ])
            .unwrap(),
        )
        .unwrap(),
        ExitCode::SUCCESS
    );
    let contract: crate::domain::BudgetContract =
        serde_json::from_str(&fs::read_to_string(&budget).unwrap()).unwrap();
    contract.validate().unwrap();

    assert_eq!(
        execute(
            Cli::try_parse_from([
                "sparkstage",
                "benchmark",
                "h3",
                "init",
                "--adapter-config",
                &adapter.display().to_string(),
                "--data-dir",
                &data_dir.display().to_string(),
                "--json",
            ])
            .unwrap(),
        )
        .unwrap(),
        ExitCode::SUCCESS
    );
    let benchmark_root = data_dir.join("benchmarks/h3");
    let run_id = fs::read_dir(&benchmark_root)
        .unwrap()
        .filter_map(Result::ok)
        .find_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            (entry.path().is_dir() && name.starts_with("H3RUN-")).then_some(name)
        })
        .expect("benchmark init should create one run");
    let evidence = directory.path().join("telemetry.csv");
    fs::write(&evidence, "timestamp,power\n0,0\n").unwrap();
    let sample = directory.path().join("sample.json");
    fs::write(
        &sample,
        serde_json::to_vec_pretty(&serde_json::json!({
            "profile": "baseline",
            "operation": "t2v",
            "seed": 7,
            "width": 960,
            "height": 544,
            "frames": 121,
            "fps": 24,
            "steps": 12,
            "elapsed_milliseconds": 240000,
            "cold_start": false,
            "job_id": "JOB-01ARZ3NDEKTSV4RRFFQ69G5FAV",
            "peak_memory_bytes": 32000000000_u64,
            "stage_milliseconds": {"dit": 180000},
            "quality_metrics": {"manual_score": 0.8},
            "evidence": [evidence],
            "notes": "offline fixture"
        }))
        .unwrap(),
    )
    .unwrap();

    for (action, extra, json) in [
        ("record", Some(("--sample", sample.as_path())), true),
        ("show", None, false),
    ] {
        let mut command = vec![
            "sparkstage".to_owned(),
            "benchmark".to_owned(),
            "h3".to_owned(),
            action.to_owned(),
            "--run".to_owned(),
            run_id.clone(),
            "--data-dir".to_owned(),
            data_dir.display().to_string(),
        ];
        if let Some((flag, path)) = extra {
            command.extend([flag.to_owned(), path.display().to_string()]);
        }
        if json {
            command.push("--json".to_owned());
        }
        assert_eq!(
            execute(Cli::try_parse_from(command).unwrap()).unwrap(),
            ExitCode::SUCCESS
        );
    }
    let report = crate::benchmark::show_h3_run(&benchmark_root, &run_id).unwrap();
    assert_eq!(report.samples.len(), 1);
    assert_eq!(
        report.samples[0].input.job_id,
        "JOB-01ARZ3NDEKTSV4RRFFQ69G5FAV"
    );
}
