use super::*;
use crate::store::write_json_atomic;

fn create_project(projects_dir: &Path, project_id: &str) -> ProjectStore {
    ProjectStore::create(
        projects_dir,
        project_id,
        "Portable Project",
        "portable brief",
        "CMD-create",
        "100",
    )
    .unwrap()
}

#[test]
fn project_verification_checks_core_identity_and_brief_hash() {
    let directory = tempfile::tempdir().unwrap();
    let projects = directory.path().join("projects");
    let store = create_project(&projects, "portable");

    let report = verify_project(&projects, "portable").unwrap();
    assert_eq!(report.project_id, "portable");
    assert_eq!(report.schema_version, PROJECT_SCHEMA_VERSION);
    assert_eq!(report.revision, 1);
    assert!(report.files >= 5);

    fs::write(store.root().join("script/brief.md"), "changed").unwrap();
    assert!(matches!(
        verify_project(&projects, "portable"),
        Err(PortabilityError::HashMismatch(path)) if path == "script/brief.md"
    ));
}

#[test]
fn archive_round_trip_verifies_hashes_and_never_overwrites() {
    let directory = tempfile::tempdir().unwrap();
    let source_projects = directory.path().join("source-projects");
    let store = create_project(&source_projects, "portable");
    let media = store.root().join("raw/S01/TAKE-demo.mp4");
    fs::create_dir_all(media.parent().unwrap()).unwrap();
    fs::write(&media, b"synthetic media bytes").unwrap();
    let archive = directory.path().join("portable.sparkstage.tar");

    let exported = export_project(&source_projects, "portable", &archive).unwrap();
    let verified = verify_archive(&archive).unwrap();
    assert_eq!(verified, exported);
    assert!(verified.files >= 6);

    let imported_projects = directory.path().join("imported-projects");
    let imported = import_project(&imported_projects, &archive).unwrap();
    assert_eq!(imported.project_id, "portable");
    assert_eq!(
        fs::read(imported_projects.join("portable/raw/S01/TAKE-demo.mp4")).unwrap(),
        b"synthetic media bytes"
    );
    assert!(matches!(
        import_project(&imported_projects, &archive),
        Err(PortabilityError::ProjectExists(id)) if id == "portable"
    ));
}

#[test]
fn archive_verification_rejects_payload_tampering() {
    let directory = tempfile::tempdir().unwrap();
    let projects = directory.path().join("projects");
    create_project(&projects, "portable");
    let archive = directory.path().join("portable.sparkstage.tar");
    export_project(&projects, "portable", &archive).unwrap();
    let mut encoded = fs::read(&archive).unwrap();
    let needle = b"portable brief";
    let offset = encoded
        .windows(needle.len())
        .position(|window| window == needle)
        .expect("brief must be stored in the uncompressed archive");
    encoded[offset] = b'P';
    fs::write(&archive, encoded).unwrap();

    assert!(matches!(
        verify_archive(&archive),
        Err(PortabilityError::HashMismatch(path)) if path == "script/brief.md"
    ));
}

#[test]
fn export_rejects_existing_and_project_internal_destinations() {
    let directory = tempfile::tempdir().unwrap();
    let projects = directory.path().join("projects");
    let store = create_project(&projects, "portable");
    let existing = directory.path().join("existing.tar");
    fs::write(&existing, b"keep").unwrap();
    assert!(matches!(
        export_project(&projects, "portable", &existing),
        Err(PortabilityError::DestinationExists(path)) if path == existing
    ));
    let internal = store.root().join("builds/archive.tar");
    assert!(matches!(
        export_project(&projects, "portable", &internal),
        Err(PortabilityError::DestinationInsideProject(path)) if path == internal
    ));
}

#[test]
fn migration_dry_run_preserves_files_and_apply_creates_backup() {
    let directory = tempfile::tempdir().unwrap();
    let projects = directory.path().join("projects");
    let store = create_project(&projects, "portable");
    set_legacy_schema(&store.root().join("project.json"));
    set_legacy_schema(&store.root().join("state.json"));
    let project_before = fs::read(store.root().join("project.json")).unwrap();
    let state_before = fs::read(store.root().join("state.json")).unwrap();

    let dry_run = plan_migration(&projects, "portable").unwrap();
    assert!(dry_run.required);
    assert!(dry_run.applicable);
    assert_eq!(dry_run.changes.len(), 2);
    assert_eq!(
        fs::read(store.root().join("project.json")).unwrap(),
        project_before
    );
    assert_eq!(
        fs::read(store.root().join("state.json")).unwrap(),
        state_before
    );

    let applied = apply_migration(&projects, "portable").unwrap();
    let backup = store.root().join(applied.backup_path.unwrap());
    assert_eq!(
        fs::read(backup.join("project.json")).unwrap(),
        project_before
    );
    assert_eq!(fs::read(backup.join("state.json")).unwrap(), state_before);
    assert!(backup.join("plan.json").is_file());
    let verified = verify_project(&projects, "portable").unwrap();
    assert_eq!(verified.schema_version, PROJECT_SCHEMA_VERSION);
    assert!(!plan_migration(&projects, "portable").unwrap().required);
}

#[test]
fn migration_reports_unknown_schema_without_writing() {
    let directory = tempfile::tempdir().unwrap();
    let projects = directory.path().join("projects");
    let store = create_project(&projects, "portable");
    let path = store.root().join("state.json");
    let mut state: serde_json::Value = read_json(&path).unwrap();
    state["schema_version"] = serde_json::Value::String("99.0".to_owned());
    write_json_atomic(&path, &state).unwrap();
    let before = fs::read(&path).unwrap();

    let plan = plan_migration(&projects, "portable").unwrap();
    assert!(plan.required);
    assert!(!plan.applicable);
    assert!(plan.backup_path.is_none());
    assert!(matches!(
        apply_migration(&projects, "portable"),
        Err(PortabilityError::MigrationUnsupported { state, .. }) if state == "99.0"
    ));
    assert_eq!(fs::read(path).unwrap(), before);
}

#[cfg(unix)]
#[test]
fn project_verification_rejects_symlinks() {
    use std::os::unix::fs::symlink;

    let directory = tempfile::tempdir().unwrap();
    let projects = directory.path().join("projects");
    let store = create_project(&projects, "portable");
    symlink("../state.json", store.root().join("raw/state-link.json")).unwrap();

    assert!(matches!(
        verify_project(&projects, "portable"),
        Err(PortabilityError::Symlink(_))
    ));
}

fn set_legacy_schema(path: &Path) {
    let mut value: serde_json::Value = read_json(path).unwrap();
    value["schema_version"] = serde_json::Value::String(LEGACY_PROJECT_SCHEMA_VERSION.to_owned());
    write_json_atomic(path, &value).unwrap();
}
