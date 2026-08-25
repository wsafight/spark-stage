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
fn worker_business_failure_returns_invalid_exit_code() {
    let reply = WorkerReply {
        protocol_version: crate::ipc::IPC_PROTOCOL_VERSION.to_owned(),
        command_id: "command".to_owned(),
        ok: false,
        revision: Some(7),
        snapshot: None,
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
