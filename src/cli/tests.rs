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
