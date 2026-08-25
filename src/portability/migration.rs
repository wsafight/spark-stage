use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use ulid::Ulid;

use super::{PortabilityError, io_error, verify_extracted_project};
use crate::domain::{PROJECT_SCHEMA_VERSION, ProjectManifest, ProjectState};
use crate::store::{
    ExclusiveFileLock, read_json, sha256_bytes, validate_project_id, write_bytes_atomic,
    write_json_atomic,
};

pub(super) const LEGACY_PROJECT_SCHEMA_VERSION: &str = "0.9";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MigrationPlan {
    pub migration_id: String,
    pub project_id: String,
    pub project_schema_version: String,
    pub state_schema_version: String,
    pub target_schema_version: String,
    pub required: bool,
    pub applicable: bool,
    pub changes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backup_path: Option<PathBuf>,
}

pub fn plan_migration(
    projects_dir: &Path,
    project_id: &str,
) -> Result<MigrationPlan, PortabilityError> {
    validate_project_id(project_id)?;
    let root = projects_dir.join(project_id);
    if !root.is_dir() {
        return Err(PortabilityError::ProjectNotFound(project_id.to_owned()));
    }
    plan_migration_at(&root, project_id)
}

pub fn apply_migration(
    projects_dir: &Path,
    project_id: &str,
) -> Result<MigrationPlan, PortabilityError> {
    validate_project_id(project_id)?;
    let root = projects_dir.join(project_id);
    if !root.is_dir() {
        return Err(PortabilityError::ProjectNotFound(project_id.to_owned()));
    }
    let _lock = ExclusiveFileLock::acquire(&root.join("project.lock"))?;
    let plan = plan_migration_at(&root, project_id)?;
    if !plan.applicable {
        return Err(PortabilityError::MigrationUnsupported {
            project: plan.project_schema_version,
            state: plan.state_schema_version,
        });
    }
    if !plan.required {
        return Ok(plan);
    }
    apply_migration_plan(&root, &plan)?;
    verify_extracted_project(&root, project_id)?;
    Ok(plan)
}

fn plan_migration_at(root: &Path, project_id: &str) -> Result<MigrationPlan, PortabilityError> {
    let project: serde_json::Value = read_json(&root.join("project.json"))?;
    let state: serde_json::Value = read_json(&root.join("state.json"))?;
    let project_version = schema_version(&project, "project.json")?;
    let state_version = schema_version(&state, "state.json")?;
    if project
        .get("project_id")
        .and_then(serde_json::Value::as_str)
        != Some(project_id)
        || state.get("project_id").and_then(serde_json::Value::as_str) != Some(project_id)
    {
        return Err(PortabilityError::ProjectIdentityMismatch);
    }
    let known = |version: &str| {
        matches!(
            version,
            PROJECT_SCHEMA_VERSION | LEGACY_PROJECT_SCHEMA_VERSION
        )
    };
    let applicable = known(&project_version) && known(&state_version);
    let required =
        project_version != PROJECT_SCHEMA_VERSION || state_version != PROJECT_SCHEMA_VERSION;
    let migration_id = format!("MIG-{}", Ulid::new());
    let mut changes = Vec::new();
    if applicable {
        if project_version != PROJECT_SCHEMA_VERSION {
            changes.push(format!(
                "project.json schema_version {project_version} -> {PROJECT_SCHEMA_VERSION}"
            ));
        }
        if state_version != PROJECT_SCHEMA_VERSION {
            changes.push(format!(
                "state.json schema_version {state_version} -> {PROJECT_SCHEMA_VERSION}"
            ));
        }
    } else {
        changes.push(format!(
            "manual repair required for project/state schemas {project_version}/{state_version}"
        ));
    }
    Ok(MigrationPlan {
        migration_id: migration_id.clone(),
        project_id: project_id.to_owned(),
        project_schema_version: project_version,
        state_schema_version: state_version,
        target_schema_version: PROJECT_SCHEMA_VERSION.to_owned(),
        required,
        applicable,
        changes,
        backup_path: (required && applicable).then(|| {
            PathBuf::from("backups")
                .join("migrations")
                .join(migration_id)
        }),
    })
}

fn apply_migration_plan(root: &Path, plan: &MigrationPlan) -> Result<(), PortabilityError> {
    let project_path = root.join("project.json");
    let state_path = root.join("state.json");
    let project_bytes =
        fs::read(&project_path).map_err(|source| io_error(&project_path, source))?;
    let state_bytes = fs::read(&state_path).map_err(|source| io_error(&state_path, source))?;
    let mut project: serde_json::Value = serde_json::from_slice(&project_bytes)?;
    let mut state: serde_json::Value = serde_json::from_slice(&state_bytes)?;
    set_schema_version(&mut project)?;
    set_schema_version(&mut state)?;
    let current_manifest: ProjectManifest = serde_json::from_value(project.clone())?;
    let current_state: ProjectState = serde_json::from_value(state.clone())?;
    current_state.validate()?;
    if current_manifest.project_id != plan.project_id || current_state.project_id != plan.project_id
    {
        return Err(PortabilityError::ProjectIdentityMismatch);
    }
    let brief_path = root.join("script/brief.md");
    let brief = fs::read(&brief_path).map_err(|source| io_error(&brief_path, source))?;
    if sha256_bytes(&brief) != current_manifest.brief_hash {
        return Err(PortabilityError::HashMismatch("script/brief.md".to_owned()));
    }
    let backup_relative = plan
        .backup_path
        .as_ref()
        .ok_or_else(|| PortabilityError::Archive("migration backup path is missing".to_owned()))?;
    let backup = root.join(backup_relative);
    fs::create_dir_all(&backup).map_err(|source| io_error(&backup, source))?;
    write_bytes_atomic(&backup.join("project.json"), &project_bytes)?;
    write_bytes_atomic(&backup.join("state.json"), &state_bytes)?;
    write_json_atomic(&backup.join("plan.json"), plan)?;
    write_json_atomic(&state_path, &state)?;
    write_json_atomic(&project_path, &project)?;
    Ok(())
}

fn schema_version(
    value: &serde_json::Value,
    subject: &'static str,
) -> Result<String, PortabilityError> {
    value
        .get("schema_version")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .ok_or(PortabilityError::SchemaMissing(subject))
}

fn set_schema_version(value: &mut serde_json::Value) -> Result<(), PortabilityError> {
    let object = value.as_object_mut().ok_or(PortabilityError::Archive(
        "migration input must be a JSON object".to_owned(),
    ))?;
    object.insert(
        "schema_version".to_owned(),
        serde_json::Value::String(PROJECT_SCHEMA_VERSION.to_owned()),
    );
    Ok(())
}
