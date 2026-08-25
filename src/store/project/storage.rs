use std::collections::BTreeSet;
use std::fs;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use ulid::Ulid;

use super::*;

pub const CLEANUP_PLAN_SCHEMA_VERSION: &str = "1.0";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CleanupPlanStatus {
    Planned,
    Applying,
    Applied,
    Restoring,
    Restored,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CleanupOperation {
    pub command_id: String,
    pub source_revision: u64,
    pub started_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CleanupItem {
    pub kind: String,
    pub subject_id: String,
    pub path: PathBuf,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CleanupPlan {
    pub schema_version: String,
    pub plan_id: String,
    pub project_id: String,
    pub source_revision: u64,
    pub status: CleanupPlanStatus,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub applied_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restored_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_operation: Option<CleanupOperation>,
    pub items: Vec<CleanupItem>,
    pub reclaimable_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StorageReport {
    pub project_id: String,
    pub total_bytes: u64,
    pub trash_bytes: u64,
    pub reclaimable_bytes: u64,
    pub reclaimable_files: usize,
}

impl ProjectStore {
    pub fn storage_report(&self) -> Result<StorageReport, StoreError> {
        let _lock = self.lock()?;
        let mut state = self.read_state()?;
        self.recover_cleanup_plans_for_state(&mut state)?;
        let items = cleanup_candidates(&self.root, &state)?;
        Ok(StorageReport {
            project_id: state.project_id,
            total_bytes: directory_bytes(&self.root)?,
            trash_bytes: directory_bytes(&self.root.join("trash"))?,
            reclaimable_bytes: items.iter().map(|item| item.bytes).sum(),
            reclaimable_files: items.len(),
        })
    }

    pub fn create_cleanup_plan(
        &self,
        expected_revision: u64,
        command_id: &str,
        now: &str,
    ) -> Result<(ProjectState, CleanupPlan), StoreError> {
        let _lock = self.lock()?;
        let mut state = self.read_state()?;
        self.recover_cleanup_plans_for_state(&mut state)?;
        ensure_revision(&state, expected_revision)?;
        let items = cleanup_candidates(&self.root, &state)?;
        let plan = CleanupPlan {
            schema_version: CLEANUP_PLAN_SCHEMA_VERSION.to_owned(),
            plan_id: format!("CLN-{}", Ulid::new()),
            project_id: state.project_id.clone(),
            source_revision: state.revision,
            status: CleanupPlanStatus::Planned,
            created_at: now.to_owned(),
            applied_at: None,
            restored_at: None,
            active_operation: None,
            reclaimable_bytes: items.iter().map(|item| item.bytes).sum(),
            items,
        };
        let plan_path = cleanup_plan_path(&self.root, &plan.plan_id)?;
        write_json_atomic(&plan_path, &plan)?;
        state.last_command_id = Some(command_id.to_owned());
        if let Err(error) = state.bump_revision(now.to_owned()) {
            let _ = fs::remove_file(&plan_path);
            return Err(error.into());
        }
        let decision =
            match self.prepare_decision("cleanup_planned", &plan.plan_id, command_id, now) {
                Ok(decision) => decision,
                Err(error) => {
                    let _ = fs::remove_file(&plan_path);
                    return Err(error);
                }
            };
        if let Err(error) = self.save_state(&state, expected_revision) {
            let _ = fs::remove_file(&plan_path);
            return Err(error);
        }
        self.commit_decisions(&[decision])?;
        Ok((state, plan))
    }

    pub fn apply_cleanup_plan(
        &self,
        plan_id: &str,
        expected_revision: u64,
        command_id: &str,
        now: &str,
    ) -> Result<(ProjectState, CleanupPlan), StoreError> {
        let _lock = self.lock()?;
        let mut state = self.read_state()?;
        self.recover_cleanup_plans_for_state(&mut state)?;
        ensure_revision(&state, expected_revision)?;
        let path = cleanup_plan_path(&self.root, plan_id)?;
        let mut plan: CleanupPlan = read_cleanup_plan(&path, plan_id, &state.project_id)?;
        if plan.status != CleanupPlanStatus::Planned {
            return Err(StoreError::InvalidCleanupPlan(format!(
                "plan `{plan_id}` is not planned"
            )));
        }
        let current = cleanup_candidates(&self.root, &state)?;
        if plan.items != current {
            return Err(StoreError::CleanupPlanStale(plan_id.to_owned()));
        }
        let decision = self.prepare_decision("cleanup_applied", plan_id, command_id, now)?;
        plan.status = CleanupPlanStatus::Applying;
        plan.active_operation = Some(CleanupOperation {
            command_id: command_id.to_owned(),
            source_revision: expected_revision,
            started_at: now.to_owned(),
        });
        write_json_atomic(&path, &plan)?;
        self.finish_cleanup_operation(&path, &mut plan, &mut state)?;
        self.commit_decisions(&[decision])?;
        Ok((state, plan))
    }

    pub fn restore_cleanup_plan(
        &self,
        plan_id: &str,
        expected_revision: u64,
        command_id: &str,
        now: &str,
    ) -> Result<(ProjectState, CleanupPlan), StoreError> {
        let _lock = self.lock()?;
        let mut state = self.read_state()?;
        self.recover_cleanup_plans_for_state(&mut state)?;
        ensure_revision(&state, expected_revision)?;
        let path = cleanup_plan_path(&self.root, plan_id)?;
        let mut plan: CleanupPlan = read_cleanup_plan(&path, plan_id, &state.project_id)?;
        if plan.status != CleanupPlanStatus::Applied {
            return Err(StoreError::InvalidCleanupPlan(format!(
                "plan `{plan_id}` is not applied"
            )));
        }
        let decision = self.prepare_decision("cleanup_restored", plan_id, command_id, now)?;
        plan.status = CleanupPlanStatus::Restoring;
        plan.active_operation = Some(CleanupOperation {
            command_id: command_id.to_owned(),
            source_revision: expected_revision,
            started_at: now.to_owned(),
        });
        write_json_atomic(&path, &plan)?;
        self.finish_cleanup_operation(&path, &mut plan, &mut state)?;
        self.commit_decisions(&[decision])?;
        Ok((state, plan))
    }

    pub fn recover_cleanup_plans(&self) -> Result<usize, StoreError> {
        let _lock = self.lock()?;
        let mut state = self.read_state()?;
        self.recover_cleanup_plans_for_state(&mut state)
    }

    pub(super) fn recover_cleanup_plans_for_state(
        &self,
        state: &mut ProjectState,
    ) -> Result<usize, StoreError> {
        let plans_directory = self.root.join("trash/plans");
        let entries = match fs::read_dir(&plans_directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(source) => return Err(io_error(&plans_directory, source)),
        };
        let mut paths = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "json")
            })
            .collect::<Vec<_>>();
        paths.sort();
        let mut recovered = 0;
        for path in paths {
            let Some(plan_id) = path.file_stem().and_then(|value| value.to_str()) else {
                continue;
            };
            let mut plan = read_cleanup_plan(&path, plan_id, &state.project_id)?;
            if !matches!(
                plan.status,
                CleanupPlanStatus::Applying | CleanupPlanStatus::Restoring
            ) {
                continue;
            }
            self.finish_cleanup_operation(&path, &mut plan, state)?;
            recovered += 1;
        }
        Ok(recovered)
    }

    fn finish_cleanup_operation(
        &self,
        path: &Path,
        plan: &mut CleanupPlan,
        state: &mut ProjectState,
    ) -> Result<(), StoreError> {
        let operation = plan.active_operation.clone().ok_or_else(|| {
            StoreError::InvalidCleanupPlan(format!(
                "interrupted plan `{}` has no active operation",
                plan.plan_id
            ))
        })?;
        let applying = match plan.status {
            CleanupPlanStatus::Applying => true,
            CleanupPlanStatus::Restoring => false,
            _ => {
                return Err(StoreError::InvalidCleanupPlan(format!(
                    "plan `{}` has no interrupted operation",
                    plan.plan_id
                )));
            }
        };
        for item in &plan.items {
            reconcile_cleanup_item(&self.root, &plan.plan_id, item, applying)?;
        }
        if state.revision == operation.source_revision {
            state.last_command_id = Some(operation.command_id.clone());
            state.bump_revision(operation.started_at.clone())?;
            write_json_atomic(&self.state_path(), state)?;
        } else if state.revision != operation.source_revision.saturating_add(1)
            || state.last_command_id.as_deref() != Some(&operation.command_id)
        {
            return Err(StoreError::InvalidCleanupPlan(format!(
                "plan `{}` expected state revision {}, found {}",
                plan.plan_id, operation.source_revision, state.revision
            )));
        }
        if applying {
            plan.status = CleanupPlanStatus::Applied;
            plan.applied_at = Some(operation.started_at);
        } else {
            plan.status = CleanupPlanStatus::Restored;
            plan.restored_at = Some(operation.started_at);
        }
        plan.active_operation = None;
        write_json_atomic(path, plan)?;
        self.recover_decisions_for_state(state)
    }
}

fn cleanup_candidates(root: &Path, state: &ProjectState) -> Result<Vec<CleanupItem>, StoreError> {
    let mut items = Vec::new();
    let mut seen = BTreeSet::new();
    for take in state.takes.values() {
        let Some(shot) = state.shots.get(&take.shot_id) else {
            continue;
        };
        let rejected = shot.rejected_take_ids.contains(&take.take_id);
        let protected = shot.selected_candidate_take_id.as_ref() == Some(&take.take_id)
            || shot.approved_take_id.as_ref() == Some(&take.take_id);
        if (!take.stale && !rejected) || protected {
            continue;
        }
        for relative in [
            Some(&take.media_path),
            take.first_frame_path.as_ref(),
            take.last_frame_path.as_ref(),
            take.handoff_candidate_path.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            push_candidate(
                root,
                &mut items,
                &mut seen,
                if rejected {
                    "rejected_take"
                } else {
                    "stale_take"
                },
                &take.take_id,
                relative,
            )?;
        }
    }
    for build in state.builds.values().filter(|build| build.stale) {
        if let Some(relative) = &build.output_path {
            push_candidate(
                root,
                &mut items,
                &mut seen,
                "stale_build",
                &build.build_id,
                relative,
            )?;
        }
    }
    items.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(items)
}

fn push_candidate(
    root: &Path,
    items: &mut Vec<CleanupItem>,
    seen: &mut BTreeSet<PathBuf>,
    kind: &str,
    subject_id: &str,
    relative: &Path,
) -> Result<(), StoreError> {
    if !safe_relative_path(relative) || !seen.insert(relative.to_owned()) {
        return Ok(());
    }
    ensure_safe_ancestors(root, relative)?;
    let path = root.join(relative);
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_file() => metadata,
        Ok(_) => return Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(source) => return Err(io_error(&path, source)),
    };
    items.push(CleanupItem {
        kind: kind.to_owned(),
        subject_id: subject_id.to_owned(),
        path: relative.to_owned(),
        bytes: metadata.len(),
    });
    Ok(())
}

fn cleanup_plan_path(root: &Path, plan_id: &str) -> Result<PathBuf, StoreError> {
    validate_plan_id(plan_id)?;
    Ok(root.join("trash/plans").join(format!("{plan_id}.json")))
}

fn cleanup_trash_path(root: &Path, plan_id: &str, relative: &Path) -> Result<PathBuf, StoreError> {
    validate_plan_id(plan_id)?;
    if !safe_relative_path(relative) {
        return Err(StoreError::InvalidCleanupPlan(format!(
            "unsafe path `{}`",
            relative.display()
        )));
    }
    Ok(root
        .join("trash")
        .join(plan_id)
        .join("files")
        .join(relative))
}

fn read_cleanup_plan(
    path: &Path,
    plan_id: &str,
    project_id: &str,
) -> Result<CleanupPlan, StoreError> {
    let plan: CleanupPlan = match read_json(path) {
        Ok(plan) => plan,
        Err(StoreError::Io { source, .. }) if source.kind() == std::io::ErrorKind::NotFound => {
            return Err(StoreError::CleanupPlanNotFound(plan_id.to_owned()));
        }
        Err(error) => return Err(error),
    };
    if plan.schema_version != CLEANUP_PLAN_SCHEMA_VERSION
        || plan.plan_id != plan_id
        || plan.project_id != project_id
        || plan
            .items
            .iter()
            .any(|item| !safe_relative_path(&item.path))
        || plan.reclaimable_bytes != plan.items.iter().map(|item| item.bytes).sum::<u64>()
        || matches!(
            plan.status,
            CleanupPlanStatus::Applying | CleanupPlanStatus::Restoring
        ) != plan.active_operation.is_some()
        || plan.active_operation.as_ref().is_some_and(|operation| {
            operation.command_id.trim().is_empty()
                || operation.source_revision == 0
                || operation.started_at.trim().is_empty()
        })
    {
        return Err(StoreError::InvalidCleanupPlan(
            "schema, identity, paths, or byte total does not match".to_owned(),
        ));
    }
    Ok(plan)
}

fn validate_plan_id(plan_id: &str) -> Result<(), StoreError> {
    let suffix = plan_id.strip_prefix("CLN-").unwrap_or_default();
    if suffix.len() == 26 && suffix.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
        Ok(())
    } else {
        Err(StoreError::InvalidCleanupPlan(format!(
            "invalid plan id `{plan_id}`"
        )))
    }
}

fn safe_relative_path(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

fn ensure_safe_ancestors(root: &Path, relative: &Path) -> Result<(), StoreError> {
    let root_metadata = fs::symlink_metadata(root).map_err(|source| io_error(root, source))?;
    if !root_metadata.file_type().is_dir() {
        return Err(StoreError::InvalidCleanupPlan(format!(
            "project root `{}` is not a regular directory",
            root.display()
        )));
    }
    let Some(parent) = relative.parent() else {
        return Ok(());
    };
    let mut current = root.to_owned();
    for component in parent.components() {
        let Component::Normal(component) = component else {
            return Err(StoreError::InvalidCleanupPlan(format!(
                "unsafe path `{}`",
                relative.display()
            )));
        };
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_dir() => {}
            Ok(_) => {
                return Err(StoreError::InvalidCleanupPlan(format!(
                    "cleanup path ancestor `{}` is not a regular directory",
                    current.display()
                )));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(source) => return Err(io_error(&current, source)),
        }
    }
    Ok(())
}

fn regular_file_bytes(root: &Path, path: &Path) -> Result<Option<u64>, StoreError> {
    let relative = path.strip_prefix(root).map_err(|_| {
        StoreError::InvalidCleanupPlan(format!(
            "cleanup path `{}` is outside the project",
            path.display()
        ))
    })?;
    if !safe_relative_path(relative) {
        return Err(StoreError::InvalidCleanupPlan(format!(
            "unsafe cleanup path `{}`",
            relative.display()
        )));
    }
    ensure_safe_ancestors(root, relative)?;
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(io_error(path, source)),
        Ok(metadata) if metadata.file_type().is_file() => Ok(Some(metadata.len())),
        Ok(_) => Err(StoreError::CleanupPathConflict(path.to_owned())),
    }
}

fn reconcile_cleanup_item(
    root: &Path,
    plan_id: &str,
    item: &CleanupItem,
    applying: bool,
) -> Result<(), StoreError> {
    let original = root.join(&item.path);
    let trashed = cleanup_trash_path(root, plan_id, &item.path)?;
    let (source, destination) = if applying {
        (&original, &trashed)
    } else {
        (&trashed, &original)
    };
    let source_bytes = regular_file_bytes(root, source)?;
    let destination_bytes = regular_file_bytes(root, destination)?;
    match (source_bytes, destination_bytes) {
        (Some(bytes), None) if bytes == item.bytes => {
            let parent = destination.parent().ok_or_else(|| {
                StoreError::InvalidCleanupPlan("cleanup destination has no parent".to_owned())
            })?;
            fs::create_dir_all(parent).map_err(|source| io_error(parent, source))?;
            let relative = destination.strip_prefix(root).map_err(|_| {
                StoreError::InvalidCleanupPlan("cleanup destination escaped project".to_owned())
            })?;
            ensure_safe_ancestors(root, relative)?;
            if regular_file_bytes(root, destination)?.is_some() {
                return Err(StoreError::CleanupPathConflict(destination.to_owned()));
            }
            fs::rename(source, destination).map_err(|source| io_error(destination, source))?;
            sync_parent(source)?;
            if source.parent() != destination.parent() {
                sync_parent(destination)?;
            }
            Ok(())
        }
        (None, Some(bytes)) if bytes == item.bytes => Ok(()),
        (Some(_), Some(_)) => Err(StoreError::CleanupPathConflict(destination.to_owned())),
        _ => Err(StoreError::CleanupPlanStale(plan_id.to_owned())),
    }
}

fn sync_parent(path: &Path) -> Result<(), StoreError> {
    let parent = path
        .parent()
        .ok_or_else(|| StoreError::InvalidCleanupPlan("cleanup path has no parent".to_owned()))?;
    fs::File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|source| io_error(parent, source))
}

fn directory_bytes(path: &Path) -> Result<u64, StoreError> {
    let entries = match fs::read_dir(path) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(source) => return Err(io_error(path, source)),
    };
    let mut bytes = 0_u64;
    for entry in entries {
        let entry = entry.map_err(|source| io_error(path, source))?;
        let metadata = entry
            .file_type()
            .map_err(|source| io_error(entry.path(), source))?;
        if metadata.is_dir() {
            bytes = bytes.saturating_add(directory_bytes(&entry.path())?);
        } else if metadata.is_file() {
            bytes = bytes.saturating_add(
                entry
                    .metadata()
                    .map_err(|source| io_error(entry.path(), source))?
                    .len(),
            );
        }
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests;
