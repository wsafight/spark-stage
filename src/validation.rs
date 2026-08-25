use std::collections::{HashMap, HashSet};
use std::path::{Component, Path};

use schemars::schema_for;
use serde::Serialize;
use serde_path_to_error::{Path as SerdePath, Segment};

use crate::domain::{Conditioning, ContinuityRelation, Operation, ScriptBundle, ShotContract};

pub const SUPPORTED_SCHEMA_VERSION: &str = "1.0";
const MAX_SHOT_DURATION_SECONDS: u32 = 15;
const MAX_AUDITION_TAKES: u8 = 4;
const DIALOGUE_CHARACTERS_PER_SECOND: f64 = 4.0;
const DIALOGUE_PUNCTUATION_PAUSE_SECONDS: f64 = 0.2;
const DIALOGUE_LINE_CHANGE_SECONDS: f64 = 0.15;
const DIALOGUE_HEAD_TAIL_MARGIN_SECONDS: f64 = 0.8;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ValidationIssue {
    pub code: &'static str,
    pub path: String,
    pub message: String,
}

#[derive(Debug)]
pub struct ValidationResult {
    pub bundle: Option<ScriptBundle>,
    pub issues: Vec<ValidationIssue>,
}

impl ValidationResult {
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.bundle.is_some() && self.issues.is_empty()
    }
}

#[must_use]
pub fn validate_json(source: &str) -> ValidationResult {
    let mut deserializer = serde_json::Deserializer::from_str(source);
    let parsed = serde_path_to_error::deserialize::<_, ScriptBundle>(&mut deserializer);

    let bundle = match parsed {
        Ok(bundle) => bundle,
        Err(error) => {
            return ValidationResult {
                bundle: None,
                issues: vec![ValidationIssue {
                    code: "JSON_CONTRACT",
                    path: serde_path_to_json_pointer(error.path()),
                    message: error.inner().to_string(),
                }],
            };
        }
    };

    if let Err(error) = deserializer.end() {
        return ValidationResult {
            bundle: None,
            issues: vec![ValidationIssue {
                code: "JSON_SYNTAX",
                path: String::new(),
                message: error.to_string(),
            }],
        };
    }

    let issues = Validator::new(&bundle).run();
    ValidationResult {
        bundle: issues.is_empty().then_some(bundle),
        issues,
    }
}

#[must_use]
pub fn json_schema() -> serde_json::Value {
    serde_json::to_value(schema_for!(ScriptBundle)).expect("schema serialization cannot fail")
}

fn serde_path_to_json_pointer(path: &SerdePath) -> String {
    let mut pointer = String::new();

    for segment in path.iter() {
        let part = match segment {
            Segment::Seq { index } => index.to_string(),
            Segment::Map { key } => key.clone(),
            Segment::Enum { variant } => variant.clone(),
            Segment::Unknown => "?".to_owned(),
        };
        pointer.push('/');
        pointer.push_str(&part.replace('~', "~0").replace('/', "~1"));
    }

    pointer
}

struct Validator<'a> {
    bundle: &'a ScriptBundle,
    issues: Vec<ValidationIssue>,
}

impl<'a> Validator<'a> {
    fn new(bundle: &'a ScriptBundle) -> Self {
        Self {
            bundle,
            issues: Vec::new(),
        }
    }

    fn run(mut self) -> Vec<ValidationIssue> {
        self.validate_project();
        self.validate_bible();
        self.validate_story();
        self.validate_shots();
        self.issues
    }

    fn validate_project(&mut self) {
        let project = &self.bundle.project;

        self.require_schema_version(&self.bundle.schema_version, "/schema_version");
        self.require_slug(&project.id, "/project/id");
        self.require_text(&project.title, "/project/title", "PROJECT_TEXT");
        self.require_text(&project.logline, "/project/logline", "PROJECT_TEXT");
        self.require_text(&project.genre, "/project/genre", "PROJECT_TEXT");
        self.require_text(&project.language, "/project/language", "PROJECT_TEXT");

        if project.target_duration_seconds == 0 {
            self.push(
                "DURATION_RANGE",
                "/project/target_duration_seconds",
                "target duration must be greater than zero",
            );
        }
        if project.shot_count == 0 {
            self.push(
                "SHOT_COUNT",
                "/project/shot_count",
                "shot count must be greater than zero",
            );
        }

        let delivery = &project.delivery;
        if delivery.width == 0
            || delivery.height == 0
            || !delivery.width.is_multiple_of(2)
            || !delivery.height.is_multiple_of(2)
        {
            self.push(
                "DELIVERY_SPEC",
                "/project/delivery",
                "width and height must be positive even numbers",
            );
        }
        if !(1..=120).contains(&delivery.fps) {
            self.push(
                "DELIVERY_SPEC",
                "/project/delivery/fps",
                "fps must be between 1 and 120",
            );
        }

        if let Some(authoring) = &self.bundle.authoring {
            self.require_text(&authoring.skill, "/authoring/skill", "AUTHORING_METADATA");
        }
    }

    fn validate_bible(&mut self) {
        let mut character_ids = HashSet::new();
        for (index, character) in self.bundle.bible.characters.iter().enumerate() {
            let base = format!("/bible/characters/{index}");
            self.require_slug(&character.id, &format!("{base}/id"));
            if !character_ids.insert(character.id.as_str()) {
                self.push(
                    "DUPLICATE_ID",
                    format!("{base}/id"),
                    format!("duplicate character id `{}`", character.id),
                );
            }
            if character.age < 18 {
                self.push(
                    "CONTENT_BOUNDARY",
                    format!("{base}/age"),
                    "short-drama characters must be adults",
                );
            }
            if !character.fictional {
                self.push(
                    "CONTENT_BOUNDARY",
                    format!("{base}/fictional"),
                    "short-drama characters must be explicitly fictional",
                );
            }
            self.require_text(&character.name, &format!("{base}/name"), "BIBLE_TEXT");
            self.require_text(
                &character.appearance,
                &format!("{base}/appearance"),
                "BIBLE_TEXT",
            );
            self.require_text(
                &character.wardrobe,
                &format!("{base}/wardrobe"),
                "BIBLE_TEXT",
            );
            self.require_text(
                &character.personality,
                &format!("{base}/personality"),
                "BIBLE_TEXT",
            );

            for (field, text) in [
                ("appearance", character.appearance.as_str()),
                ("wardrobe", character.wardrobe.as_str()),
            ] {
                self.reject_prohibited_story_terms(text, &format!("{base}/{field}"));
            }
        }

        let mut location_ids = HashSet::new();
        for (index, location) in self.bundle.bible.locations.iter().enumerate() {
            let base = format!("/bible/locations/{index}");
            self.require_slug(&location.id, &format!("{base}/id"));
            if !location_ids.insert(location.id.as_str()) {
                self.push(
                    "DUPLICATE_ID",
                    format!("{base}/id"),
                    format!("duplicate location id `{}`", location.id),
                );
            }
            self.require_text(&location.name, &format!("{base}/name"), "BIBLE_TEXT");
            self.require_text(
                &location.description,
                &format!("{base}/description"),
                "BIBLE_TEXT",
            );
        }

        self.require_text(&self.bundle.bible.style, "/bible/style", "BIBLE_TEXT");
    }

    fn validate_story(&mut self) {
        self.require_text(&self.bundle.story.synopsis, "/story/synopsis", "STORY_TEXT");
        self.reject_prohibited_story_terms(&self.bundle.story.synopsis, "/story/synopsis");

        if self.bundle.story.beats.is_empty() {
            self.push(
                "STORY_BEATS",
                "/story/beats",
                "story must contain at least one beat",
            );
        }
        for (index, beat) in self.bundle.story.beats.iter().enumerate() {
            self.require_text(beat, &format!("/story/beats/{index}"), "STORY_TEXT");
            self.reject_prohibited_story_terms(beat, &format!("/story/beats/{index}"));
        }
    }

    fn validate_shots(&mut self) {
        let project = &self.bundle.project;
        if self.bundle.shots.len() != project.shot_count {
            self.push(
                "SHOT_COUNT",
                "/shots",
                format!(
                    "project declares {} shots but bundle contains {}",
                    project.shot_count,
                    self.bundle.shots.len()
                ),
            );
        }

        let total_duration = self
            .bundle
            .shots
            .iter()
            .fold(0_u32, |total, shot| total.saturating_add(shot.duration));
        if total_duration != project.target_duration_seconds {
            self.push(
                "DURATION_MISMATCH",
                "/shots",
                format!(
                    "shot durations total {total_duration}s but project target is {}s",
                    project.target_duration_seconds
                ),
            );
        }

        let character_ids: HashSet<&str> = self
            .bundle
            .bible
            .characters
            .iter()
            .map(|character| character.id.as_str())
            .collect();
        let location_ids: HashSet<&str> = self
            .bundle
            .bible
            .locations
            .iter()
            .map(|location| location.id.as_str())
            .collect();
        let shot_indices: HashMap<&str, usize> = self
            .bundle
            .shots
            .iter()
            .enumerate()
            .map(|(index, shot)| (shot.id.as_str(), index))
            .collect();
        let mut seen_shots = HashSet::new();

        for (index, shot) in self.bundle.shots.iter().enumerate() {
            let base = format!("/shots/{index}");
            self.require_schema_version(&shot.schema_version, &format!("{base}/schema_version"));
            if !is_shot_id(&shot.id) {
                self.push(
                    "INVALID_ID",
                    format!("{base}/id"),
                    "shot id must use S followed by at least two digits",
                );
            }
            if !seen_shots.insert(shot.id.as_str()) {
                self.push(
                    "DUPLICATE_ID",
                    format!("{base}/id"),
                    format!("duplicate shot id `{}`", shot.id),
                );
            }

            self.validate_shot(index, shot, &character_ids, &location_ids, &shot_indices);
        }
    }

    fn validate_shot(
        &mut self,
        index: usize,
        shot: &ShotContract,
        character_ids: &HashSet<&str>,
        location_ids: &HashSet<&str>,
        shot_indices: &HashMap<&str, usize>,
    ) {
        let base = format!("/shots/{index}");
        self.require_text(&shot.title, &format!("{base}/title"), "SHOT_TEXT");
        self.require_text(&shot.prompt, &format!("{base}/prompt"), "SHOT_TEXT");
        self.reject_backend_controls(&shot.prompt, &format!("{base}/prompt"));
        self.reject_prohibited_story_terms(&shot.prompt, &format!("{base}/prompt"));

        if !(1..=MAX_SHOT_DURATION_SECONDS).contains(&shot.duration) {
            self.push(
                "DURATION_RANGE",
                format!("{base}/duration"),
                format!("shot duration must be between 1 and {MAX_SHOT_DURATION_SECONDS} seconds"),
            );
        }

        let delivery = &self.bundle.project.delivery;
        if shot.width != delivery.width
            || shot.height != delivery.height
            || shot.fps != delivery.fps
        {
            self.push(
                "DELIVERY_MISMATCH",
                base.clone(),
                "shot width, height, and fps must match project delivery",
            );
        }

        let mut shot_characters = HashSet::new();
        for (character_index, character_id) in shot.characters.iter().enumerate() {
            if !character_ids.contains(character_id.as_str()) {
                self.push(
                    "UNKNOWN_CHARACTER",
                    format!("{base}/characters/{character_index}"),
                    format!("character `{character_id}` is not declared in bible"),
                );
            }
            if !shot_characters.insert(character_id.as_str()) {
                self.push(
                    "DUPLICATE_ID",
                    format!("{base}/characters/{character_index}"),
                    format!("character `{character_id}` is listed more than once"),
                );
            }
        }

        if !location_ids.contains(shot.location.as_str()) {
            self.push(
                "UNKNOWN_LOCATION",
                format!("{base}/location"),
                format!("location `{}` is not declared in bible", shot.location),
            );
        }

        for character_id in shot.camera.screen_direction.keys() {
            if !shot_characters.contains(character_id.as_str()) {
                self.push(
                    "CHARACTER_SCOPE",
                    format!("{base}/camera/screen_direction/{character_id}"),
                    "screen direction character must be listed in shot.characters",
                );
            }
        }

        self.validate_conditioning(index, shot.operation, shot.conditioning.as_ref());
        self.validate_continuity(index, shot, &shot_characters, shot_indices);
        self.validate_dialogue(index, shot, &shot_characters);

        if shot.generation_plan.audition_takes > MAX_AUDITION_TAKES {
            self.push(
                "AUDITION_BUDGET",
                format!("{base}/generation_plan/audition_takes"),
                format!("audition_takes cannot exceed {MAX_AUDITION_TAKES}"),
            );
        }
        self.require_text(
            &shot.generation_plan.audition_profile,
            &format!("{base}/generation_plan/audition_profile"),
            "PROFILE_NAME",
        );
        self.require_text(
            &shot.generation_plan.final_profile,
            &format!("{base}/generation_plan/final_profile"),
            "PROFILE_NAME",
        );
        if shot.generation_plan.audition_profile == shot.generation_plan.final_profile {
            self.push(
                "PROFILE_COLLISION",
                format!("{base}/generation_plan/final_profile"),
                "final_profile must differ from audition_profile so take cost and lineage remain distinguishable",
            );
        }
    }

    fn validate_conditioning(
        &mut self,
        shot_index: usize,
        operation: Operation,
        conditioning: Option<&Conditioning>,
    ) {
        let base = format!("/shots/{shot_index}/conditioning");

        match (operation, conditioning) {
            (Operation::T2v, None) => {}
            (Operation::T2v, Some(_)) => self.push(
                "CONDITIONING_MISMATCH",
                &base,
                "t2v must not include visual conditioning",
            ),
            (Operation::I2v, Some(value))
                if value.first_frame.is_some() && value.last_frame.is_none() => {}
            (Operation::I2v, _) => self.push(
                "CONDITIONING_MISMATCH",
                &base,
                "i2v requires first_frame and must not include last_frame",
            ),
            (Operation::Flf2v, Some(value))
                if value.first_frame.is_some() && value.last_frame.is_some() => {}
            (Operation::Flf2v, _) => self.push(
                "CONDITIONING_MISMATCH",
                &base,
                "flf2v requires first_frame and last_frame",
            ),
            (Operation::R2v, Some(value))
                if !value.reference_images.is_empty() || value.reference_video.is_some() => {}
            (Operation::R2v, _) => self.push(
                "CONDITIONING_MISMATCH",
                &base,
                "r2v requires reference_images or reference_video",
            ),
        }

        if let Some(value) = conditioning {
            for (field, path) in [
                (value.first_frame.as_deref(), "first_frame"),
                (value.last_frame.as_deref(), "last_frame"),
                (value.reference_video.as_deref(), "reference_video"),
            ] {
                if let Some(file) = field {
                    self.require_reference_path(file, &format!("{base}/{path}"));
                }
            }
            for (index, file) in value.reference_images.iter().enumerate() {
                self.require_reference_path(file, &format!("{base}/reference_images/{index}"));
            }
        }
    }

    fn validate_continuity(
        &mut self,
        shot_index: usize,
        shot: &ShotContract,
        shot_characters: &HashSet<&str>,
        shot_indices: &HashMap<&str, usize>,
    ) {
        let base = format!("/shots/{shot_index}/continuity");
        let source_index = shot.continuity.from.as_deref().and_then(|source| {
            let Some(source_index) = shot_indices.get(source).copied() else {
                self.push(
                    "CONTINUITY_REFERENCE",
                    format!("{base}/from"),
                    format!("continuity source `{source}` does not exist"),
                );
                return None;
            };
            if source_index >= shot_index {
                self.push(
                    "CONTINUITY_REFERENCE",
                    format!("{base}/from"),
                    "continuity source must appear before the current shot",
                );
                return None;
            }
            Some(source_index)
        });

        if shot.continuity.relation == ContinuityRelation::Continuous
            && shot.continuity.from.is_none()
        {
            self.push(
                "CONTINUITY_REFERENCE",
                format!("{base}/from"),
                "continuous relation requires a source shot",
            );
        }

        for (field, states) in [
            ("state_in", &shot.continuity.state_in),
            ("state_out", &shot.continuity.state_out),
        ] {
            for key in states.keys() {
                if let Some((owner, _)) = key.split_once('.')
                    && !shot_characters.contains(owner)
                {
                    self.push(
                        "CHARACTER_SCOPE",
                        format!("{base}/{field}/{}", escape_pointer(key)),
                        format!("continuity character `{owner}` must be listed in shot.characters"),
                    );
                }
            }
        }

        if shot.continuity.relation == ContinuityRelation::Continuous
            && let Some(source_index) = source_index
        {
            let source = &self.bundle.shots[source_index];
            if source.location != shot.location {
                self.push(
                    "CONTINUITY_STATE_MISMATCH",
                    format!("{base}/relation"),
                    "continuous shots must use the same location",
                );
            }
            for (key, expected) in &shot.continuity.state_in {
                if let Some(actual) = source.continuity.state_out.get(key)
                    && actual != expected
                {
                    self.push(
                        "CONTINUITY_STATE_MISMATCH",
                        format!("{base}/state_in/{}", escape_pointer(key)),
                        format!(
                            "state `{key}` is `{expected}` but source shot ends with `{actual}`"
                        ),
                    );
                }
            }
        }
    }

    fn validate_dialogue(
        &mut self,
        shot_index: usize,
        shot: &ShotContract,
        shot_characters: &HashSet<&str>,
    ) {
        let base = format!("/shots/{shot_index}/dialogue");
        for (line_index, line) in shot.dialogue.iter().enumerate() {
            if !shot_characters.contains(line.who.as_str()) {
                self.push(
                    "CHARACTER_SCOPE",
                    format!("{base}/{line_index}/who"),
                    format!(
                        "dialogue speaker `{}` must be listed in shot.characters",
                        line.who
                    ),
                );
            }
            self.require_text(
                &line.text,
                &format!("{base}/{line_index}/text"),
                "DIALOGUE_TEXT",
            );
            self.reject_prohibited_story_terms(&line.text, &format!("{base}/{line_index}/text"));
        }

        let estimated = estimate_dialogue_seconds(shot);
        let available = f64::from(shot.duration) - DIALOGUE_HEAD_TAIL_MARGIN_SECONDS;
        if estimated > available.max(0.0) {
            self.push(
                "DIALOGUE_BUDGET",
                base,
                format!(
                    "estimated dialogue is {estimated:.2}s but only {:.2}s is available after breathing room",
                    available.max(0.0)
                ),
            );
        }
    }

    fn require_schema_version(&mut self, version: &str, path: &str) {
        if version != SUPPORTED_SCHEMA_VERSION {
            self.push(
                "SCHEMA_VERSION",
                path,
                format!(
                    "unsupported schema version `{version}`; expected `{SUPPORTED_SCHEMA_VERSION}`"
                ),
            );
        }
    }

    fn require_slug(&mut self, value: &str, path: &str) {
        if !is_slug(value) {
            self.push(
                "INVALID_ID",
                path,
                "id must contain lowercase ASCII letters, digits, and single hyphens",
            );
        }
    }

    fn require_text(&mut self, value: &str, path: &str, code: &'static str) {
        if value.trim().is_empty() || value.trim() == "..." {
            self.push(code, path, "text must contain a concrete value");
        }
    }

    fn require_reference_path(&mut self, value: &str, path: &str) {
        let candidate = Path::new(value);
        let is_relative_safe = !value.is_empty()
            && !candidate.is_absolute()
            && candidate
                .components()
                .all(|component| matches!(component, Component::Normal(_)));
        let starts_with_refs = candidate
            .components()
            .next()
            .is_some_and(|component| component.as_os_str() == "refs");

        if !is_relative_safe || !starts_with_refs {
            self.push(
                "REFERENCE_PATH",
                path,
                "reference path must be a safe project-relative path under refs/",
            );
        }
    }

    fn reject_backend_controls(&mut self, value: &str, path: &str) {
        let normalized = value.to_ascii_lowercase();
        let forbidden = [
            "comfyui",
            "workflow json",
            "node_id",
            "node id",
            "scheduler:",
            "sampler:",
            "steps:",
        ];
        if let Some(term) = forbidden.iter().find(|term| normalized.contains(**term)) {
            self.push(
                "BACKEND_FIELD",
                path,
                format!("prompt contains backend control `{term}`"),
            );
        }
    }

    fn reject_prohibited_story_terms(&mut self, value: &str, path: &str) {
        let normalized = value.to_ascii_lowercase();
        let prohibited = ["未成年", "校服", "童装", "真实公众人物", "public figure"];
        if let Some(term) = prohibited.iter().find(|term| normalized.contains(**term)) {
            self.push(
                "CONTENT_BOUNDARY",
                path,
                format!("text contains prohibited short-drama term `{term}`"),
            );
        }
    }

    fn push(&mut self, code: &'static str, path: impl Into<String>, message: impl Into<String>) {
        self.issues.push(ValidationIssue {
            code,
            path: path.into(),
            message: message.into(),
        });
    }
}

fn is_slug(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('-')
        && !value.ends_with('-')
        && !value.contains("--")
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn is_shot_id(value: &str) -> bool {
    value
        .strip_prefix('S')
        .is_some_and(|digits| digits.len() >= 2 && digits.bytes().all(|byte| byte.is_ascii_digit()))
}

fn estimate_dialogue_seconds(shot: &ShotContract) -> f64 {
    let mut spoken_characters = 0_u32;
    let mut punctuation = 0_u32;

    for line in &shot.dialogue {
        for character in line.text.chars() {
            if is_pause_punctuation(character) {
                punctuation += 1;
            } else if !character.is_whitespace() {
                spoken_characters += 1;
            }
        }
    }

    let line_changes = shot.dialogue.len().saturating_sub(1) as f64;
    f64::from(spoken_characters) / DIALOGUE_CHARACTERS_PER_SECOND
        + f64::from(punctuation) * DIALOGUE_PUNCTUATION_PAUSE_SECONDS
        + line_changes * DIALOGUE_LINE_CHANGE_SECONDS
}

fn is_pause_punctuation(character: char) -> bool {
    matches!(
        character,
        '，' | '。' | '！' | '？' | '；' | '：' | '、' | ',' | '.' | '!' | '?' | ';' | ':'
    )
}

fn escape_pointer(value: &str) -> String {
    value.replace('~', "~0").replace('/', "~1")
}

#[cfg(test)]
mod tests;
