use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::*;
use crate::domain::{ReferenceAsset, ReferenceSubjectKind, ShotStage};
use crate::store::{sha256_file, write_bytes_atomic};

const MAX_REFERENCE_BYTES: u64 = 100 * 1024 * 1024;
const ALLOWED_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "webp"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReferenceImpact {
    pub subject_kind: ReferenceSubjectKind,
    pub subject_id: String,
    pub affected_shot_ids: Vec<String>,
    pub affected_take_ids: Vec<String>,
    pub affected_build_ids: Vec<String>,
}

impl ReferenceImpact {
    #[must_use]
    pub fn has_production_artifacts(&self) -> bool {
        !self.affected_take_ids.is_empty() || !self.affected_build_ids.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReferenceVerification {
    pub references: usize,
    pub active_references: usize,
    pub bytes: u64,
}

#[derive(Debug, Clone, Copy)]
pub struct ReferenceWriteRequest<'a> {
    pub source: &'a Path,
    pub accept_impact: bool,
    pub expected_revision: u64,
    pub command_id: &'a str,
    pub now: &'a str,
}

impl ProjectStore {
    pub fn reference_impact(
        &self,
        subject_kind: ReferenceSubjectKind,
        subject_id: &str,
    ) -> Result<ReferenceImpact, StoreError> {
        let state = self.read_state()?;
        let bundle = self
            .read_active_bundle()?
            .ok_or(StoreError::NoActiveContract)?;
        ensure_subject_exists(&bundle, subject_kind, subject_id)?;
        let affected_shot_ids = affected_shots(&bundle, subject_kind, subject_id);
        let changed = affected_shot_ids.iter().cloned().collect::<HashSet<_>>();
        let affected_take_ids = state
            .takes
            .values()
            .filter(|take| !take.stale && changed.contains(&take.shot_id))
            .map(|take| take.take_id.clone())
            .collect();
        let affected_build_ids = self.build_ids_for_shots(&state, &changed);
        Ok(ReferenceImpact {
            subject_kind,
            subject_id: subject_id.to_owned(),
            affected_shot_ids,
            affected_take_ids,
            affected_build_ids,
        })
    }

    pub fn import_reference(
        &self,
        subject_kind: ReferenceSubjectKind,
        subject_id: &str,
        request: ReferenceWriteRequest<'_>,
    ) -> Result<(ProjectState, ReferenceAsset, ReferenceImpact), StoreError> {
        self.write_reference(subject_kind, subject_id, None, request)
    }

    pub fn replace_reference(
        &self,
        reference_id: &str,
        request: ReferenceWriteRequest<'_>,
    ) -> Result<(ProjectState, ReferenceAsset, ReferenceImpact), StoreError> {
        let state = self.read_state()?;
        let current = state
            .references
            .get(reference_id)
            .filter(|reference| reference.active)
            .ok_or_else(|| StoreError::ReferenceNotFound(reference_id.to_owned()))?;
        self.write_reference(
            current.subject_kind,
            &current.subject_id,
            Some(reference_id),
            request,
        )
    }

    fn write_reference(
        &self,
        subject_kind: ReferenceSubjectKind,
        subject_id: &str,
        supersedes: Option<&str>,
        request: ReferenceWriteRequest<'_>,
    ) -> Result<(ProjectState, ReferenceAsset, ReferenceImpact), StoreError> {
        let ReferenceWriteRequest {
            source,
            accept_impact,
            expected_revision,
            command_id,
            now,
        } = request;
        let _lock = self.lock()?;
        let mut state = self.read_state()?;
        if state.revision != expected_revision {
            return Err(StoreError::RevisionConflict {
                expected: expected_revision,
                actual: state.revision,
            });
        }
        let bundle = self
            .read_active_bundle()?
            .ok_or(StoreError::NoActiveContract)?;
        ensure_subject_exists(&bundle, subject_kind, subject_id)?;
        if let Some(reference_id) = supersedes {
            let current = state
                .references
                .get(reference_id)
                .filter(|reference| reference.active)
                .ok_or_else(|| StoreError::ReferenceNotFound(reference_id.to_owned()))?;
            if current.subject_kind != subject_kind || current.subject_id != subject_id {
                return Err(StoreError::ReferenceNotFound(reference_id.to_owned()));
            }
        }
        let impact = self.reference_impact_unlocked(&state, &bundle, subject_kind, subject_id);
        if impact.has_production_artifacts() && !accept_impact {
            return Err(StoreError::ReferenceImpactConfirmationRequired);
        }
        reject_active_work(&state, &impact)?;
        let source_file_name = source
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .ok_or_else(|| StoreError::InvalidReferenceSource("file name is missing".to_owned()))?;
        let extension = source
            .extension()
            .and_then(|value| value.to_str())
            .map(str::to_ascii_lowercase)
            .filter(|value| ALLOWED_EXTENSIONS.contains(&value.as_str()))
            .ok_or_else(|| {
                StoreError::InvalidReferenceSource(
                    "expected a jpg, jpeg, png, or webp file".to_owned(),
                )
            })?;
        let metadata = fs::symlink_metadata(source).map_err(|error| io_error(source, error))?;
        if !metadata.file_type().is_file()
            || metadata.file_type().is_symlink()
            || metadata.len() == 0
            || metadata.len() > MAX_REFERENCE_BYTES
        {
            return Err(StoreError::InvalidReferenceSource(format!(
                "source must be a non-empty regular file no larger than {MAX_REFERENCE_BYTES} bytes"
            )));
        }
        let bytes = fs::read(source).map_err(|error| io_error(source, error))?;
        if u64::try_from(bytes.len()).ok() != Some(metadata.len()) {
            return Err(StoreError::InvalidReferenceSource(
                "source changed while it was being read".to_owned(),
            ));
        }
        let command_hash = sha256_bytes(command_id.as_bytes());
        let reference_id = format!("REF-{}", &command_hash[..26]);
        let kind = subject_kind_name(subject_kind);
        let relative_path = PathBuf::from("refs")
            .join(kind)
            .join(subject_id)
            .join(format!("{reference_id}.{extension}"));
        let destination = self.root.join(&relative_path);
        let asset = ReferenceAsset {
            reference_id: reference_id.clone(),
            subject_kind,
            subject_id: subject_id.to_owned(),
            relative_path,
            sha256: sha256_bytes(&bytes),
            bytes: metadata.len(),
            original_file_name: source_file_name.to_owned(),
            active: true,
            supersedes: supersedes.map(str::to_owned),
            superseded_by: None,
            created_at: now.to_owned(),
        };
        if let Some(previous_id) = supersedes {
            state
                .references
                .get_mut(previous_id)
                .expect("validated reference must exist")
                .active = false;
            state
                .references
                .get_mut(previous_id)
                .expect("validated reference must exist")
                .superseded_by = Some(reference_id.clone());
        }
        invalidate_impact(&mut state, &impact);
        state.references.insert(reference_id.clone(), asset.clone());
        state.last_command_id = Some(command_id.to_owned());
        state.bump_revision(now.to_owned())?;
        let decision = self.prepare_decision(
            if supersedes.is_some() {
                "reference_replaced"
            } else {
                "reference_imported"
            },
            &reference_id,
            command_id,
            now,
        )?;
        write_bytes_atomic(&destination, &bytes)?;
        if let Err(error) = self.save_state(&state, expected_revision) {
            let _ = fs::remove_file(&destination);
            return Err(error);
        }
        self.commit_decisions(&[decision])?;
        Ok((state, asset, impact))
    }

    fn reference_impact_unlocked(
        &self,
        state: &ProjectState,
        bundle: &ScriptBundle,
        subject_kind: ReferenceSubjectKind,
        subject_id: &str,
    ) -> ReferenceImpact {
        let affected_shot_ids = affected_shots(bundle, subject_kind, subject_id);
        let changed = affected_shot_ids.iter().cloned().collect::<HashSet<_>>();
        let affected_take_ids = state
            .takes
            .values()
            .filter(|take| !take.stale && changed.contains(&take.shot_id))
            .map(|take| take.take_id.clone())
            .collect();
        let affected_build_ids = self.build_ids_for_shots(state, &changed);
        ReferenceImpact {
            subject_kind,
            subject_id: subject_id.to_owned(),
            affected_shot_ids,
            affected_take_ids,
            affected_build_ids,
        }
    }

    pub fn verify_references(&self) -> Result<ReferenceVerification, StoreError> {
        let state = self.read_state()?;
        let mut bytes = 0_u64;
        for reference in state.references.values() {
            let path = self.root.join(&reference.relative_path);
            let metadata = fs::symlink_metadata(&path).map_err(|error| io_error(&path, error))?;
            if !metadata.file_type().is_file()
                || metadata.file_type().is_symlink()
                || metadata.len() != reference.bytes
                || sha256_file(&path)? != reference.sha256
            {
                return Err(StoreError::ReferenceIntegrity(
                    reference.reference_id.clone(),
                ));
            }
            bytes = bytes
                .checked_add(reference.bytes)
                .ok_or_else(|| StoreError::InvalidReferenceSource("size overflow".to_owned()))?;
        }
        Ok(ReferenceVerification {
            references: state.references.len(),
            active_references: state
                .references
                .values()
                .filter(|reference| reference.active)
                .count(),
            bytes,
        })
    }
}

fn ensure_subject_exists(
    bundle: &ScriptBundle,
    kind: ReferenceSubjectKind,
    id: &str,
) -> Result<(), StoreError> {
    let exists = match kind {
        ReferenceSubjectKind::Character => bundle.bible.characters.iter().any(|item| item.id == id),
        ReferenceSubjectKind::Location => bundle.bible.locations.iter().any(|item| item.id == id),
    };
    if exists {
        Ok(())
    } else {
        Err(StoreError::ReferenceSubjectNotFound {
            kind: subject_kind_name(kind).to_owned(),
            id: id.to_owned(),
        })
    }
}

fn affected_shots(bundle: &ScriptBundle, kind: ReferenceSubjectKind, id: &str) -> Vec<String> {
    bundle
        .shots
        .iter()
        .filter(|shot| match kind {
            ReferenceSubjectKind::Character => shot.characters.iter().any(|value| value == id),
            ReferenceSubjectKind::Location => shot.location == id,
        })
        .map(|shot| shot.id.clone())
        .collect()
}

fn reject_active_work(state: &ProjectState, impact: &ReferenceImpact) -> Result<(), StoreError> {
    for shot_id in &impact.affected_shot_ids {
        if let Some(job_id) = state
            .shots
            .get(shot_id)
            .and_then(|shot| shot.active_job_id.clone())
        {
            return Err(StoreError::ShotBusy {
                shot_id: shot_id.clone(),
                job_id,
            });
        }
    }
    if let Some(build) = state
        .builds
        .values()
        .find(|build| matches!(build.status.as_str(), "queued" | "running"))
    {
        return Err(StoreError::BuildBusy {
            build_id: build.build_id.clone(),
        });
    }
    Ok(())
}

fn invalidate_impact(state: &mut ProjectState, impact: &ReferenceImpact) {
    let invalidated_shots = impact
        .affected_take_ids
        .iter()
        .filter_map(|take_id| state.takes.get(take_id).map(|take| take.shot_id.clone()))
        .collect::<HashSet<_>>();
    for take in state.takes.values_mut() {
        if invalidated_shots.contains(&take.shot_id) {
            take.stale = true;
        }
    }
    for shot_id in &invalidated_shots {
        if let Some(shot) = state.shots.get_mut(shot_id) {
            shot.stage = ShotStage::Pending;
            shot.selected_candidate_take_id = None;
            shot.approved_take_id = None;
            shot.audition_target_takes = None;
            shot.stale = true;
        }
    }
    state.pending_approvals.retain(|approval| {
        approval
            .shot_id
            .as_ref()
            .is_none_or(|shot_id| !invalidated_shots.contains(shot_id))
    });
    super::builds::mark_build_ids_stale(state, &impact.affected_build_ids);
}

pub fn subject_kind_name(kind: ReferenceSubjectKind) -> &'static str {
    match kind {
        ReferenceSubjectKind::Character => "character",
        ReferenceSubjectKind::Location => "location",
    }
}

#[must_use]
pub fn reference_subject_keys(shot: &crate::domain::ShotContract) -> Vec<String> {
    let mut subjects = shot
        .characters
        .iter()
        .map(|id| format!("character:{id}"))
        .collect::<Vec<_>>();
    subjects.push(format!("location:{}", shot.location));
    subjects.sort();
    subjects.dedup();
    subjects
}

#[must_use]
pub fn active_reference_fingerprint(state: &ProjectState, subjects: &[String]) -> String {
    let mut entries = state
        .references
        .values()
        .filter(|reference| reference.active)
        .filter(|reference| {
            let subject = format!(
                "{}:{}",
                subject_kind_name(reference.subject_kind),
                reference.subject_id
            );
            subjects.binary_search(&subject).is_ok()
        })
        .map(|reference| format!("{}:{}", reference.reference_id, reference.sha256))
        .collect::<Vec<_>>();
    entries.sort();
    sha256_bytes(entries.join("\n").as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::TakeMetadata;
    use crate::validation::validate_json;

    const BUNDLE: &str =
        include_str!("../../../skills/screenwriter/examples/valid-short-drama.json");

    fn active_store() -> (tempfile::TempDir, ProjectStore) {
        let directory = tempfile::tempdir().unwrap();
        let mut bundle = validate_json(BUNDLE).bundle.unwrap();
        bundle.shots[1].characters = vec!["lin".to_owned()];
        let store = ProjectStore::create(
            directory.path(),
            &bundle.project.id,
            &bundle.project.title,
            "brief",
            "create",
            "100",
        )
        .unwrap();
        let (_, approval) = store.apply_bundle(&bundle, 1, "apply", "101").unwrap();
        store
            .approve_script(Some(&approval.approval_id), 2, "approve", "102")
            .unwrap();
        (directory, store)
    }

    fn take(id: &str, shot_id: &str) -> TakeMetadata {
        TakeMetadata {
            take_id: id.to_owned(),
            shot_id: shot_id.to_owned(),
            job_id: format!("JOB-{id}"),
            profile: "audition".to_owned(),
            status: "candidate".to_owned(),
            media_path: PathBuf::from(format!("raw/{shot_id}/{id}.mp4")),
            input_hash: "input".to_owned(),
            adapter_fingerprint: "adapter".to_owned(),
            workflow_hash: "workflow".to_owned(),
            model_fingerprint: "model".to_owned(),
            seed: 1,
            elapsed_milliseconds: 1,
            first_frame_path: None,
            last_frame_path: None,
            handoff_candidate_path: None,
            parent_take_id: None,
            promotion_strategy: None,
            hard_checks: Vec::new(),
            warnings: Vec::new(),
            stale: false,
        }
    }

    #[test]
    fn import_is_immutable_and_verifiable_without_invalidating_empty_shots() {
        let (_directory, store) = active_store();
        let source_dir = tempfile::tempdir().unwrap();
        let source = source_dir.path().join("portrait.png");
        fs::write(&source, b"reference-one").unwrap();

        let (state, reference, impact) = store
            .import_reference(
                ReferenceSubjectKind::Character,
                "zhao",
                ReferenceWriteRequest {
                    source: &source,
                    accept_impact: false,
                    expected_revision: 3,
                    command_id: "import-reference",
                    now: "103",
                },
            )
            .unwrap();

        assert_eq!(impact.affected_shot_ids, ["S01"]);
        assert!(impact.affected_take_ids.is_empty());
        assert!(!state.shots["S01"].stale);
        assert_eq!(
            fs::read(store.root().join(&reference.relative_path)).unwrap(),
            b"reference-one"
        );
        assert_eq!(store.verify_references().unwrap().active_references, 1);

        fs::write(store.root().join(&reference.relative_path), b"tampered").unwrap();
        assert!(matches!(
            store.verify_references(),
            Err(StoreError::ReferenceIntegrity(id)) if id == reference.reference_id
        ));
    }

    #[test]
    fn replacement_requires_confirmation_and_invalidates_only_dependent_takes() {
        let (_directory, store) = active_store();
        let source_dir = tempfile::tempdir().unwrap();
        let first = source_dir.path().join("first.jpg");
        let second = source_dir.path().join("second.jpg");
        fs::write(&first, b"first-reference").unwrap();
        fs::write(&second, b"second-reference").unwrap();
        let (mut state, reference, _) = store
            .import_reference(
                ReferenceSubjectKind::Character,
                "zhao",
                ReferenceWriteRequest {
                    source: &first,
                    accept_impact: false,
                    expected_revision: 3,
                    command_id: "import",
                    now: "103",
                },
            )
            .unwrap();
        for (shot_id, take_id) in [("S01", "TAKE-one"), ("S02", "TAKE-two")] {
            state
                .takes
                .insert(take_id.to_owned(), take(take_id, shot_id));
            state
                .shots
                .get_mut(shot_id)
                .unwrap()
                .take_ids
                .push(take_id.to_owned());
        }
        state.bump_revision("104".to_owned()).unwrap();
        store.save_state(&state, 4).unwrap();

        let error = store
            .replace_reference(
                &reference.reference_id,
                ReferenceWriteRequest {
                    source: &second,
                    accept_impact: false,
                    expected_revision: 5,
                    command_id: "replace-denied",
                    now: "105",
                },
            )
            .unwrap_err();
        assert!(matches!(
            error,
            StoreError::ReferenceImpactConfirmationRequired
        ));
        let (changed, replacement, impact) = store
            .replace_reference(
                &reference.reference_id,
                ReferenceWriteRequest {
                    source: &second,
                    accept_impact: true,
                    expected_revision: 5,
                    command_id: "replace-accepted",
                    now: "106",
                },
            )
            .unwrap();

        assert_eq!(impact.affected_take_ids, ["TAKE-one"]);
        assert!(changed.takes["TAKE-one"].stale);
        assert!(!changed.takes["TAKE-two"].stale);
        assert!(changed.shots["S01"].stale);
        assert!(!changed.shots["S02"].stale);
        assert!(!changed.references[&reference.reference_id].active);
        assert_eq!(replacement.supersedes, Some(reference.reference_id));
    }
}
