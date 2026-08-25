use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Serialize;

use super::*;
use crate::domain::{BibleEntry, BibleIndex, JobState, ShotRuntimeState};

#[derive(Serialize)]
pub(super) struct JournalEntry<'a> {
    pub(super) event_id: String,
    pub(super) kind: &'a str,
    pub(super) subject_id: &'a str,
    pub(super) command_id: &'a str,
    pub(super) occurred_at: &'a str,
}

pub(super) fn initial_shot_state(
    shot_id: String,
    title: String,
    risk: crate::domain::Risk,
) -> ShotRuntimeState {
    ShotRuntimeState {
        shot_id,
        title,
        stage: ShotStage::Pending,
        risk,
        active_job_id: None,
        audition_target_takes: None,
        selected_candidate_take_id: None,
        approved_take_id: None,
        take_ids: Vec::new(),
        rejected_take_ids: Vec::new(),
        fail_codes: Vec::new(),
        stale: false,
    }
}

pub(super) fn ensure_revision(
    state: &ProjectState,
    expected_revision: u64,
) -> Result<(), StoreError> {
    if state.revision == expected_revision {
        Ok(())
    } else {
        Err(StoreError::RevisionConflict {
            expected: expected_revision,
            actual: state.revision,
        })
    }
}

pub(super) fn validate_job(
    job: &JobJournal,
    project_id: &str,
    command_id: &str,
) -> Result<(), StoreError> {
    validate_job_id(&job.job_id)?;
    if job.schema_version != crate::domain::PROJECT_SCHEMA_VERSION
        || job.project_id != project_id
        || job.command_id != command_id
        || job.state != JobState::Queued
        || !job.attempts.is_empty()
        || job.shot_id.trim().is_empty()
        || job.reserved_take_id.trim().is_empty()
    {
        return Err(StoreError::InvalidJob(
            "new job identity, state, or ownership does not match the enqueue command".to_owned(),
        ));
    }
    Ok(())
}

pub(super) fn validate_job_id(value: &str) -> Result<(), StoreError> {
    let suffix = value.strip_prefix("JOB-").unwrap_or_default();
    if suffix.len() == 26 && suffix.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
        Ok(())
    } else {
        Err(StoreError::InvalidJob(format!("invalid job id `{value}`")))
    }
}

pub(super) fn validate_take_id(value: &str) -> Result<(), StoreError> {
    let suffix = value.strip_prefix("TAKE-").unwrap_or_default();
    if suffix.len() == 26 && suffix.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
        Ok(())
    } else {
        Err(StoreError::InvalidJob(format!("invalid take id `{value}`")))
    }
}

pub(super) fn render_contract(
    root: &Path,
    bundle: &ScriptBundle,
    receipt: &AuthoringReceipt,
) -> Result<(), StoreError> {
    let project_markdown = format!(
        "# {}\n\n{}\n\n- Genre: {}\n- Language: {}\n- Duration: {} seconds\n- Shots: {}\n",
        bundle.project.title,
        bundle.project.logline,
        bundle.project.genre,
        bundle.project.language,
        bundle.project.target_duration_seconds,
        bundle.project.shot_count
    );
    write_text_atomic(&root.join("PROJECT.md"), &project_markdown)?;
    write_text_atomic(&root.join("bible/style.md"), &bundle.bible.style)?;

    let mut characters = BTreeMap::new();
    for character in &bundle.bible.characters {
        let relative = PathBuf::from(format!("bible/characters/{}.md", character.id));
        let markdown = format!(
            "# {}\n\n- Age: {}\n- Fictional: {}\n\n## Appearance\n\n{}\n\n## Wardrobe\n\n{}\n\n## Personality\n\n{}\n",
            character.name,
            character.age,
            character.fictional,
            character.appearance,
            character.wardrobe,
            character.personality
        );
        write_text_atomic(&root.join(&relative), &markdown)?;
        characters.insert(
            character.id.clone(),
            BibleEntry {
                source: relative,
                references: Vec::new(),
            },
        );
    }

    let mut locations = BTreeMap::new();
    for location in &bundle.bible.locations {
        let relative = PathBuf::from(format!("bible/locations/{}.md", location.id));
        write_text_atomic(
            &root.join(&relative),
            &format!("# {}\n\n{}\n", location.name, location.description),
        )?;
        locations.insert(
            location.id.clone(),
            BibleEntry {
                source: relative,
                references: Vec::new(),
            },
        );
    }
    write_json_atomic(
        &root.join("bible/index.json"),
        &BibleIndex {
            schema_version: crate::domain::PROJECT_SCHEMA_VERSION.to_owned(),
            characters,
            locations,
            style_source: PathBuf::from("bible/style.md"),
        },
    )?;
    write_json_atomic(&root.join("script/bundle.json"), bundle)?;
    write_json_atomic(&root.join("script/shots.json"), &bundle.shots)?;
    write_json_atomic(&root.join("script/authoring.json"), receipt)?;
    let story = format!(
        "# Story\n\n{}\n\n## Beats\n\n{}\n",
        bundle.story.synopsis,
        bundle
            .story
            .beats
            .iter()
            .map(|beat| format!("- {beat}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    write_text_atomic(&root.join("script/story.md"), &story)
}

pub fn validate_project_id(value: &str) -> Result<(), StoreError> {
    let valid = !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && value
            .as_bytes()
            .last()
            .is_some_and(u8::is_ascii_alphanumeric)
        && !value.contains("--");
    if valid {
        Ok(())
    } else {
        Err(StoreError::InvalidProjectId(value.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_id_accepts_documented_slug_boundaries() {
        assert!(validate_project_id("a").is_ok());
        assert!(validate_project_id("spark-stage-2026").is_ok());
        assert!(validate_project_id(&"a".repeat(64)).is_ok());
    }

    #[test]
    fn project_id_rejects_ambiguous_or_unsafe_slugs() {
        for project_id in [
            "",
            "SparkStage",
            "spark_stage",
            "spark--stage",
            "-sparkstage",
            "sparkstage-",
            "spark/stage",
        ] {
            assert!(validate_project_id(project_id).is_err(), "{project_id}");
        }
        assert!(validate_project_id(&"a".repeat(65)).is_err());
    }
}
