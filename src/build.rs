use std::collections::HashSet;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};

use crate::domain::{ProjectState, ScriptBundle};
use crate::media::{MediaCheckStatus, MediaReport};

mod contact_sheet;
mod executor;

pub(crate) use executor::{BuildEvent, BuildExecutorHandle, BuildRequest};

pub const BUILD_RECIPE_SCHEMA_VERSION: &str = "1.0";
pub const BUILD_REVIEW_REPORT_SCHEMA_VERSION: &str = "1.0";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BuildKind {
    Draft,
    Trailer,
    Final,
}

impl BuildKind {
    pub fn parse(value: &str) -> Result<Self, BuildError> {
        match value {
            "draft" => Ok(Self::Draft),
            "trailer" => Ok(Self::Trailer),
            "final" => Ok(Self::Final),
            _ => Err(BuildError::Kind(value.to_owned())),
        }
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Trailer => "trailer",
            Self::Final => "final",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildRecipe {
    pub schema_version: String,
    pub build_id: String,
    pub project_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contract_id: Option<String>,
    pub contract_hash: String,
    pub source_revision: u64,
    pub kind: BuildKind,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub expected_duration_seconds: u32,
    pub inputs: Vec<BuildInput>,
    pub output_path: PathBuf,
    pub delivery_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildInput {
    pub shot_id: String,
    pub take_id: String,
    pub media_path: PathBuf,
    pub profile: String,
    pub input_hash: String,
    pub adapter_fingerprint: String,
    pub workflow_hash: String,
    pub model_fingerprint: String,
    pub seed: u64,
    #[serde(default)]
    pub warnings: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_frame_path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trim_seconds: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildReviewReport {
    pub schema_version: String,
    pub build_id: String,
    pub project_id: String,
    pub kind: BuildKind,
    pub recipe_path: PathBuf,
    pub output_path: PathBuf,
    pub delivery_path: PathBuf,
    pub contact_sheet_path: PathBuf,
    pub recipe: BuildRecipe,
    pub media: MediaReport,
    pub human_visual_review_required: bool,
}

pub fn plan(
    build_id: &str,
    kind: BuildKind,
    state: &ProjectState,
    bundle: &ScriptBundle,
) -> Result<BuildRecipe, BuildError> {
    plan_selected(build_id, kind, state, bundle, &[])
}

pub fn plan_selected(
    build_id: &str,
    kind: BuildKind,
    state: &ProjectState,
    bundle: &ScriptBundle,
    selected_shot_ids: &[String],
) -> Result<BuildRecipe, BuildError> {
    let selected = selected_shot_ids
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    if selected.len() != selected_shot_ids.len() {
        return Err(BuildError::DuplicateShotSelection);
    }
    for shot_id in &selected {
        if !bundle.shots.iter().any(|shot| shot.id == *shot_id) {
            return Err(BuildError::ShotSelectionMissing((*shot_id).to_owned()));
        }
    }
    let mut inputs = Vec::with_capacity(bundle.shots.len());
    let mut expected_duration_seconds = 0_u32;
    for shot in &bundle.shots {
        if !selected.is_empty() && !selected.contains(shot.id.as_str()) {
            continue;
        }
        let runtime = state
            .shots
            .get(&shot.id)
            .ok_or_else(|| BuildError::ShotMissing(shot.id.clone()))?;
        let take_id = match kind {
            BuildKind::Draft => runtime.selected_candidate_take_id.as_ref(),
            BuildKind::Trailer | BuildKind::Final => runtime.approved_take_id.as_ref(),
        }
        .ok_or_else(|| BuildError::TakeDecisionMissing {
            shot_id: shot.id.clone(),
            decision: match kind {
                BuildKind::Draft => "selected",
                BuildKind::Trailer | BuildKind::Final => "approved",
            },
        })?;
        let take = state
            .takes
            .get(take_id)
            .ok_or_else(|| BuildError::TakeMissing(take_id.clone()))?;
        if take.shot_id != shot.id {
            return Err(BuildError::TakeShotMismatch {
                take_id: take_id.clone(),
                expected_shot_id: shot.id.clone(),
                actual_shot_id: take.shot_id.clone(),
            });
        }
        if runtime.rejected_take_ids.contains(take_id) {
            return Err(BuildError::TakeRejected(take_id.clone()));
        }
        if take.stale {
            return Err(BuildError::TakeStale(take_id.clone()));
        }
        let first_frame_path = take
            .first_frame_path
            .as_ref()
            .ok_or_else(|| BuildError::FirstFrameMissing(take_id.clone()))?;
        if kind != BuildKind::Draft && take.profile != shot.generation_plan.final_profile {
            return Err(BuildError::FinalProfileRequired {
                shot_id: shot.id.clone(),
                take_id: take_id.clone(),
                expected: shot.generation_plan.final_profile.clone(),
                actual: take.profile.clone(),
            });
        }
        validate_relative_path(&take.media_path)?;
        validate_relative_path(first_frame_path)?;
        let trim_seconds = (kind == BuildKind::Trailer).then_some(2_u32.min(shot.duration));
        expected_duration_seconds = expected_duration_seconds
            .checked_add(trim_seconds.unwrap_or(shot.duration))
            .ok_or(BuildError::DurationOverflow)?;
        inputs.push(BuildInput {
            shot_id: shot.id.clone(),
            take_id: take_id.clone(),
            media_path: take.media_path.clone(),
            profile: take.profile.clone(),
            input_hash: take.input_hash.clone(),
            adapter_fingerprint: take.adapter_fingerprint.clone(),
            workflow_hash: take.workflow_hash.clone(),
            model_fingerprint: take.model_fingerprint.clone(),
            seed: take.seed,
            warnings: take.warnings.clone(),
            first_frame_path: Some(first_frame_path.clone()),
            trim_seconds,
        });
    }
    if inputs.is_empty() {
        return Err(BuildError::Empty);
    }
    let output_path = PathBuf::from("builds").join(build_id).join("output.mp4");
    let delivery_path = match kind {
        BuildKind::Draft => PathBuf::from("review/draft-cut.mp4"),
        BuildKind::Trailer => {
            PathBuf::from("final").join(format!("{}-trailer.mp4", state.project_id))
        }
        BuildKind::Final => PathBuf::from("final").join(format!("{}.mp4", state.project_id)),
    };
    Ok(BuildRecipe {
        schema_version: BUILD_RECIPE_SCHEMA_VERSION.to_owned(),
        build_id: build_id.to_owned(),
        project_id: state.project_id.clone(),
        contract_id: state.active_contract_id.clone(),
        contract_hash: crate::store::sha256_json(bundle)
            .map_err(|error| BuildError::Recipe(error.to_string()))?,
        source_revision: state.revision,
        kind,
        width: bundle.project.delivery.width,
        height: bundle.project.delivery.height,
        fps: bundle.project.delivery.fps,
        expected_duration_seconds,
        inputs,
        output_path,
        delivery_path,
    })
}

pub fn missing_runtime_capabilities() -> Result<Vec<String>, BuildError> {
    let requirements: [(&str, &str, &[&str]); 4] = [
        ("demuxer", "-demuxers", &["mov"]),
        ("muxer", "-muxers", &["mp4", "image2"]),
        ("encoder", "-encoders", &["libx264", "aac", "mjpeg"]),
        (
            "filter",
            "-filters",
            &[
                "aresample",
                "asetpts",
                "atrim",
                "blackdetect",
                "concat",
                "fps",
                "freezedetect",
                "loudnorm",
                "pad",
                "scale",
                "setpts",
                "setsar",
                "silencedetect",
                "trim",
                "xstack",
            ],
        ),
    ];
    let mut missing = Vec::new();
    for (kind, argument, names) in requirements {
        let listing = command_listing(argument)?;
        for name in names {
            if !listing_contains(&listing, name) {
                missing.push(format!("{kind}:{name}"));
            }
        }
    }
    Ok(missing)
}

pub fn run(project_root: &Path, recipe: &BuildRecipe) -> Result<(), BuildError> {
    validate_recipe(recipe)?;
    let output = project_path(project_root, &recipe.output_path)?;
    let delivery = project_path(project_root, &recipe.delivery_path)?;
    let output_parent = output
        .parent()
        .ok_or_else(|| BuildError::Path(recipe.output_path.clone()))?;
    fs::create_dir_all(output_parent).map_err(|source| BuildError::Io {
        path: output_parent.to_owned(),
        source,
    })?;
    if let Some(parent) = delivery.parent() {
        fs::create_dir_all(parent).map_err(|source| BuildError::Io {
            path: parent.to_owned(),
            source,
        })?;
    }
    let staging = output.with_file_name(".output.tmp.mp4");
    if staging.exists() {
        fs::remove_file(&staging).map_err(|source| BuildError::Io {
            path: staging.clone(),
            source,
        })?;
    }

    let mut command = Command::new("ffmpeg");
    command.args(["-hide_banner", "-loglevel", "error", "-nostdin", "-y"]);
    for input in &recipe.inputs {
        command
            .arg("-i")
            .arg(project_path(project_root, &input.media_path)?);
    }
    command
        .arg("-filter_complex")
        .arg(filter_graph(recipe))
        .args([
            "-map",
            "[vout]",
            "-map",
            "[aout]",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "aac",
            "-movflags",
            "+faststart",
        ])
        .arg(&staging)
        .stdout(Stdio::null());
    let result = command
        .output()
        .map_err(|source| BuildError::Command(source.to_string()))?;
    if !result.status.success() {
        return Err(BuildError::Ffmpeg(last_line(&result.stderr)));
    }
    let bytes = fs::metadata(&staging)
        .map_err(|source| BuildError::Io {
            path: staging.clone(),
            source,
        })?
        .len();
    if bytes == 0 {
        return Err(BuildError::Ffmpeg(
            "ffmpeg produced an empty file".to_owned(),
        ));
    }
    let report = crate::media::inspect(&staging, recipe.expected_duration_seconds, true)
        .map_err(|error| BuildError::MediaInspection(error.to_string()))?;
    validate_output_report(recipe, &report)?;
    fs::rename(&staging, &output).map_err(|source| BuildError::Io {
        path: output.clone(),
        source,
    })?;
    publish_copy(&output, &delivery)?;
    let contact_sheet_path = contact_sheet::create(project_root, recipe)?;
    let report_path = project_root
        .join("builds")
        .join(&recipe.build_id)
        .join("review-report.json");
    crate::store::write_json_atomic(
        &report_path,
        &BuildReviewReport {
            schema_version: BUILD_REVIEW_REPORT_SCHEMA_VERSION.to_owned(),
            build_id: recipe.build_id.clone(),
            project_id: recipe.project_id.clone(),
            kind: recipe.kind,
            recipe_path: PathBuf::from("builds")
                .join(&recipe.build_id)
                .join("recipe.json"),
            output_path: recipe.output_path.clone(),
            delivery_path: recipe.delivery_path.clone(),
            contact_sheet_path,
            recipe: recipe.clone(),
            media: report,
            human_visual_review_required: true,
        },
    )
    .map_err(|error| BuildError::Report(error.to_string()))
}

pub fn validate_current(recipe: &BuildRecipe, state: &ProjectState) -> Result<(), BuildError> {
    if recipe.project_id != state.project_id {
        return Err(BuildError::Stale("project identity changed".to_owned()));
    }
    if let Some(contract_id) = recipe.contract_id.as_deref() {
        if state.active_contract_id.as_deref() != Some(contract_id) {
            return Err(BuildError::Stale("active contract changed".to_owned()));
        }
        let contract = state
            .contracts
            .get(contract_id)
            .ok_or_else(|| BuildError::Stale("source contract is missing".to_owned()))?;
        if contract.bundle_hash != recipe.contract_hash {
            return Err(BuildError::Stale("source contract hash changed".to_owned()));
        }
    }
    for input in &recipe.inputs {
        let take = state
            .takes
            .get(&input.take_id)
            .ok_or_else(|| BuildError::Stale(format!("take `{}` is missing", input.take_id)))?;
        if take.stale
            || take.shot_id != input.shot_id
            || take.media_path != input.media_path
            || take.profile != input.profile
            || take.input_hash != input.input_hash
            || take.adapter_fingerprint != input.adapter_fingerprint
            || take.workflow_hash != input.workflow_hash
            || take.model_fingerprint != input.model_fingerprint
            || take.seed != input.seed
            || take.first_frame_path != input.first_frame_path
        {
            return Err(BuildError::Stale(format!(
                "take `{}` lineage changed",
                input.take_id
            )));
        }
        let shot = state
            .shots
            .get(&input.shot_id)
            .ok_or_else(|| BuildError::Stale(format!("shot `{}` is missing", input.shot_id)))?;
        let current_take_id = match recipe.kind {
            BuildKind::Draft => shot.selected_candidate_take_id.as_deref(),
            BuildKind::Trailer | BuildKind::Final => shot.approved_take_id.as_deref(),
        };
        if current_take_id != Some(input.take_id.as_str()) {
            return Err(BuildError::Stale(format!(
                "shot `{}` decision changed",
                input.shot_id
            )));
        }
    }
    Ok(())
}

fn validate_output_report(recipe: &BuildRecipe, report: &MediaReport) -> Result<(), BuildError> {
    let mut failures = report
        .checks
        .iter()
        .filter(|check| check.status == MediaCheckStatus::Fail)
        .map(|check| check.code.clone())
        .collect::<Vec<_>>();
    if report.width != recipe.width || report.height != recipe.height {
        failures.push(format!(
            "FRAME_SIZE(actual={}x{},expected={}x{})",
            report.width, report.height, recipe.width, recipe.height
        ));
    }
    if (report.fps - f64::from(recipe.fps)).abs() > 0.01 {
        failures.push(format!(
            "FRAME_RATE(actual={:.3},expected={})",
            report.fps, recipe.fps
        ));
    }
    if failures.is_empty() && report.valid {
        Ok(())
    } else {
        if failures.is_empty() {
            failures.push("MEDIA_REPORT_INVALID".to_owned());
        }
        Err(BuildError::OutputInvalid(failures.join(", ")))
    }
}

fn validate_recipe(recipe: &BuildRecipe) -> Result<(), BuildError> {
    if recipe.schema_version != BUILD_RECIPE_SCHEMA_VERSION
        || recipe.inputs.is_empty()
        || recipe.width == 0
        || recipe.height == 0
        || recipe.fps == 0
    {
        return Err(BuildError::Recipe("invalid recipe header".to_owned()));
    }
    validate_relative_path(&recipe.output_path)?;
    validate_relative_path(&recipe.delivery_path)?;
    for input in &recipe.inputs {
        validate_relative_path(&input.media_path)?;
        let first_frame_path = input
            .first_frame_path
            .as_ref()
            .ok_or_else(|| BuildError::FirstFrameMissing(input.take_id.clone()))?;
        validate_relative_path(first_frame_path)?;
    }
    Ok(())
}

fn filter_graph(recipe: &BuildRecipe) -> String {
    let mut filters = Vec::with_capacity(recipe.inputs.len() * 2 + 1);
    let mut concat_inputs = String::new();
    for (index, input) in recipe.inputs.iter().enumerate() {
        let trim_video = input.trim_seconds.map_or_else(String::new, |seconds| {
            format!("trim=duration={seconds},setpts=PTS-STARTPTS,")
        });
        let trim_audio = input.trim_seconds.map_or_else(String::new, |seconds| {
            format!("atrim=duration={seconds},asetpts=PTS-STARTPTS,")
        });
        filters.push(format!(
            "[{index}:v:0]{trim_video}scale={}:{}:force_original_aspect_ratio=decrease,pad={}:{}:(ow-iw)/2:(oh-ih)/2,fps={},setsar=1[v{index}]",
            recipe.width, recipe.height, recipe.width, recipe.height, recipe.fps
        ));
        filters.push(format!(
            "[{index}:a:0]{trim_audio}aresample=48000,loudnorm=I=-16:LRA=11:TP=-1.5[a{index}]"
        ));
        concat_inputs.push_str(&format!("[v{index}][a{index}]"));
    }
    filters.push(format!(
        "{concat_inputs}concat=n={}:v=1:a=1[vout][aout]",
        recipe.inputs.len()
    ));
    filters.join(";")
}

fn publish_copy(source: &Path, destination: &Path) -> Result<(), BuildError> {
    let temporary = destination.with_file_name(format!(
        ".{}.tmp",
        destination
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("delivery.mp4")
    ));
    fs::copy(source, &temporary).map_err(|source| BuildError::Io {
        path: temporary.clone(),
        source,
    })?;
    fs::rename(&temporary, destination).map_err(|source| BuildError::Io {
        path: destination.to_owned(),
        source,
    })
}

fn project_path(root: &Path, relative: &Path) -> Result<PathBuf, BuildError> {
    validate_relative_path(relative)?;
    Ok(root.join(relative))
}

fn validate_relative_path(path: &Path) -> Result<(), BuildError> {
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_) | Component::CurDir))
    {
        Err(BuildError::Path(path.to_owned()))
    } else {
        Ok(())
    }
}

fn command_listing(argument: &str) -> Result<String, BuildError> {
    let output = Command::new("ffmpeg")
        .args(["-hide_banner", argument])
        .output()
        .map_err(|source| BuildError::Command(source.to_string()))?;
    if !output.status.success() {
        return Err(BuildError::Ffmpeg(last_line(&output.stderr)));
    }
    let mut listing = String::from_utf8_lossy(&output.stdout).into_owned();
    listing.push_str(&String::from_utf8_lossy(&output.stderr));
    Ok(listing)
}

fn listing_contains(listing: &str, expected: &str) -> bool {
    listing.lines().any(|line| {
        line.split_whitespace()
            .any(|token| token.split(',').any(|name| name == expected))
    })
}

fn last_line(value: &[u8]) -> String {
    String::from_utf8_lossy(value)
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("ffmpeg failed without diagnostics")
        .trim()
        .to_owned()
}

#[derive(Debug, thiserror::Error)]
pub enum BuildError {
    #[error("unsupported build kind `{0}`")]
    Kind(String),
    #[error("build has no input shots")]
    Empty,
    #[error("build shot selection contains duplicate IDs")]
    DuplicateShotSelection,
    #[error("selected shot `{0}` is missing from the active contract")]
    ShotSelectionMissing(String),
    #[error("shot `{0}` is missing from project state")]
    ShotMissing(String),
    #[error("shot `{shot_id}` has no {decision} take")]
    TakeDecisionMissing {
        shot_id: String,
        decision: &'static str,
    },
    #[error("take `{0}` is missing from project state")]
    TakeMissing(String),
    #[error("take `{take_id}` belongs to shot `{actual_shot_id}`, expected `{expected_shot_id}`")]
    TakeShotMismatch {
        take_id: String,
        expected_shot_id: String,
        actual_shot_id: String,
    },
    #[error("take `{0}` was rejected")]
    TakeRejected(String),
    #[error("take `{0}` is stale")]
    TakeStale(String),
    #[error("take `{0}` has no extracted first frame")]
    FirstFrameMissing(String),
    #[error(
        "shot `{shot_id}` take `{take_id}` uses profile `{actual}`, expected final profile `{expected}`"
    )]
    FinalProfileRequired {
        shot_id: String,
        take_id: String,
        expected: String,
        actual: String,
    },
    #[error("build duration overflow")]
    DurationOverflow,
    #[error("unsafe build path `{0}`")]
    Path(PathBuf),
    #[error("invalid build recipe: {0}")]
    Recipe(String),
    #[error("cannot access `{path}`: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("cannot start ffmpeg: {0}")]
    Command(String),
    #[error("ffmpeg build failed: {0}")]
    Ffmpeg(String),
    #[error("cannot inspect build output: {0}")]
    MediaInspection(String),
    #[error("build output failed media checks: {0}")]
    OutputInvalid(String),
    #[error("cannot write build review report: {0}")]
    Report(String),
    #[error("build inputs became stale: {0}")]
    Stale(String),
    #[error("cannot create contact sheet: {0}")]
    ContactSheet(String),
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::TryRecvError;

    use super::*;
    use crate::domain::{ProjectState, Risk, ShotRuntimeState, ShotStage, TakeMetadata};

    const BUNDLE: &str = include_str!("../skills/screenwriter/examples/valid-short-drama.json");

    fn planning_fixture(profile: &str) -> (ProjectState, ScriptBundle) {
        let bundle: ScriptBundle = serde_json::from_str(BUNDLE).unwrap();
        let mut state = ProjectState::new(
            bundle.project.id.clone(),
            bundle.project.title.clone(),
            "100".to_owned(),
        );
        for (index, shot) in bundle.shots.iter().enumerate() {
            let take_id = format!("TAKE-{index:026}");
            state.takes.insert(
                take_id.clone(),
                TakeMetadata {
                    take_id: take_id.clone(),
                    shot_id: shot.id.clone(),
                    job_id: format!("JOB-{index:026}"),
                    profile: profile.to_owned(),
                    status: "candidate".to_owned(),
                    media_path: PathBuf::from(format!("raw/{}/{take_id}.mp4", shot.id)),
                    input_hash: "input".to_owned(),
                    adapter_fingerprint: "adapter".to_owned(),
                    workflow_hash: "workflow".to_owned(),
                    model_fingerprint: "model".to_owned(),
                    seed: u64::try_from(index).unwrap(),
                    elapsed_milliseconds: 100,
                    first_frame_path: Some(PathBuf::from(format!(
                        "review/{}/{}-first.jpg",
                        shot.id, take_id
                    ))),
                    last_frame_path: None,
                    handoff_candidate_path: None,
                    parent_take_id: None,
                    promotion_strategy: None,
                    hard_checks: Vec::new(),
                    warnings: Vec::new(),
                    stale: false,
                },
            );
            state.shots.insert(
                shot.id.clone(),
                ShotRuntimeState {
                    shot_id: shot.id.clone(),
                    title: shot.title.clone(),
                    stage: ShotStage::Selected,
                    risk: Risk::Low,
                    active_job_id: None,
                    audition_target_takes: None,
                    selected_candidate_take_id: Some(take_id.clone()),
                    approved_take_id: (profile == shot.generation_plan.final_profile)
                        .then_some(take_id.clone()),
                    take_ids: vec![take_id],
                    rejected_take_ids: Vec::new(),
                    fail_codes: Vec::new(),
                    stale: false,
                },
            );
        }
        (state, bundle)
    }

    #[test]
    fn trailer_filter_trims_each_input_before_concat() {
        let recipe = BuildRecipe {
            schema_version: BUILD_RECIPE_SCHEMA_VERSION.to_owned(),
            build_id: "BLD-test".to_owned(),
            project_id: "demo".to_owned(),
            contract_id: None,
            contract_hash: "contract".to_owned(),
            source_revision: 1,
            kind: BuildKind::Trailer,
            width: 960,
            height: 544,
            fps: 24,
            expected_duration_seconds: 4,
            inputs: vec![
                BuildInput {
                    shot_id: "S01".to_owned(),
                    take_id: "T01".to_owned(),
                    media_path: PathBuf::from("raw/S01/T01.mp4"),
                    profile: "final".to_owned(),
                    input_hash: "input-1".to_owned(),
                    adapter_fingerprint: "adapter".to_owned(),
                    workflow_hash: "workflow".to_owned(),
                    model_fingerprint: "model".to_owned(),
                    seed: 1,
                    warnings: Vec::new(),
                    first_frame_path: None,
                    trim_seconds: Some(2),
                },
                BuildInput {
                    shot_id: "S02".to_owned(),
                    take_id: "T02".to_owned(),
                    media_path: PathBuf::from("raw/S02/T02.mp4"),
                    profile: "final".to_owned(),
                    input_hash: "input-2".to_owned(),
                    adapter_fingerprint: "adapter".to_owned(),
                    workflow_hash: "workflow".to_owned(),
                    model_fingerprint: "model".to_owned(),
                    seed: 2,
                    warnings: Vec::new(),
                    first_frame_path: None,
                    trim_seconds: Some(2),
                },
            ],
            output_path: PathBuf::from("builds/BLD-test/output.mp4"),
            delivery_path: PathBuf::from("final/demo-trailer.mp4"),
        };

        let graph = filter_graph(&recipe);

        assert_eq!(graph.matches("trim=duration=2").count(), 4);
        assert!(graph.contains("concat=n=2:v=1:a=1[vout][aout]"));
        assert!(!graph.contains("raw/S01"));
    }

    #[test]
    fn output_report_requires_hard_checks_dimensions_and_frame_rate() {
        let (state, bundle) = planning_fixture("audition");
        let recipe = plan("BLD-test", BuildKind::Draft, &state, &bundle).unwrap();
        let mut report = MediaReport {
            valid: true,
            duration_seconds: f64::from(recipe.expected_duration_seconds),
            fps: f64::from(recipe.fps),
            width: recipe.width,
            height: recipe.height,
            audio_channels: Some(2),
            checks: vec![crate::media::MediaCheck {
                code: "DECODE_OK".to_owned(),
                status: MediaCheckStatus::Pass,
                detail: "ok".to_owned(),
            }],
        };
        validate_output_report(&recipe, &report).unwrap();

        report.width -= 1;
        let error = validate_output_report(&recipe, &report).unwrap_err();
        assert!(error.to_string().contains("FRAME_SIZE"));

        report.width = recipe.width;
        report.checks[0].status = MediaCheckStatus::Fail;
        let error = validate_output_report(&recipe, &report).unwrap_err();
        assert!(error.to_string().contains("DECODE_OK"));
    }

    #[test]
    fn draft_recipe_uses_only_explicit_selected_takes() {
        let (state, bundle) = planning_fixture("audition");

        let recipe = plan("BLD-test", BuildKind::Draft, &state, &bundle).unwrap();

        assert_eq!(recipe.source_revision, state.revision);
        assert_eq!(
            recipe.contract_hash,
            crate::store::sha256_json(&bundle).unwrap()
        );
        assert_eq!(recipe.inputs.len(), bundle.shots.len());
        assert_eq!(
            recipe.expected_duration_seconds,
            bundle.project.target_duration_seconds
        );
        assert!(
            recipe
                .inputs
                .iter()
                .all(|input| input.trim_seconds.is_none())
        );
        assert!(
            recipe
                .inputs
                .iter()
                .all(|input| input.adapter_fingerprint == "adapter"
                    && input.workflow_hash == "workflow"
                    && input.model_fingerprint == "model")
        );
        validate_current(&recipe, &state).unwrap();
    }

    #[test]
    fn scoped_draft_uses_contract_order_and_rejects_invalid_selection() {
        let (state, bundle) = planning_fixture("audition");

        let recipe = plan_selected(
            "BLD-scoped",
            BuildKind::Draft,
            &state,
            &bundle,
            &["S02".to_owned()],
        )
        .unwrap();

        assert_eq!(recipe.inputs.len(), 1);
        assert_eq!(recipe.inputs[0].shot_id, "S02");
        assert_eq!(recipe.expected_duration_seconds, bundle.shots[1].duration);
        validate_current(&recipe, &state).unwrap();
        assert!(matches!(
            plan_selected(
                "BLD-duplicate",
                BuildKind::Draft,
                &state,
                &bundle,
                &["S01".to_owned(), "S01".to_owned()]
            ),
            Err(BuildError::DuplicateShotSelection)
        ));
        assert!(matches!(
            plan_selected(
                "BLD-missing",
                BuildKind::Draft,
                &state,
                &bundle,
                &["S99".to_owned()]
            ),
            Err(BuildError::ShotSelectionMissing(id)) if id == "S99"
        ));
    }

    #[test]
    fn current_validation_rejects_changed_take_decision() {
        let (mut state, bundle) = planning_fixture("audition");
        let recipe = plan("BLD-test", BuildKind::Draft, &state, &bundle).unwrap();
        state
            .shots
            .get_mut("S01")
            .unwrap()
            .selected_candidate_take_id = Some("TAKE-other".to_owned());

        let error = validate_current(&recipe, &state).unwrap_err();

        assert!(matches!(error, BuildError::Stale(_)));
    }

    #[test]
    fn build_planning_requires_a_safe_extracted_first_frame() {
        let (mut state, bundle) = planning_fixture("audition");
        let take_id = state.shots["S01"]
            .selected_candidate_take_id
            .clone()
            .unwrap();
        state.takes.get_mut(&take_id).unwrap().first_frame_path = None;
        assert!(matches!(
            plan("BLD-test", BuildKind::Draft, &state, &bundle),
            Err(BuildError::FirstFrameMissing(id)) if id == take_id
        ));

        state.takes.get_mut(&take_id).unwrap().first_frame_path =
            Some(PathBuf::from("../outside.jpg"));
        assert!(matches!(
            plan("BLD-test", BuildKind::Draft, &state, &bundle),
            Err(BuildError::Path(_))
        ));
    }

    #[test]
    fn final_recipe_rejects_audition_profile_even_if_marked_approved() {
        let (mut state, bundle) = planning_fixture("audition");
        for shot in state.shots.values_mut() {
            shot.approved_take_id = shot.selected_candidate_take_id.clone();
        }

        let error = plan("BLD-test", BuildKind::Final, &state, &bundle).unwrap_err();

        assert!(matches!(error, BuildError::FinalProfileRequired { .. }));
    }

    #[test]
    fn recipe_rejects_cross_shot_and_unsafe_take_paths() {
        let (mut state, bundle) = planning_fixture("audition");
        let take_id = state.shots["S01"]
            .selected_candidate_take_id
            .clone()
            .unwrap();
        state.takes.get_mut(&take_id).unwrap().shot_id = "S02".to_owned();
        assert!(matches!(
            plan("BLD-test", BuildKind::Draft, &state, &bundle),
            Err(BuildError::TakeShotMismatch { .. })
        ));

        state.takes.get_mut(&take_id).unwrap().shot_id = "S01".to_owned();
        state.takes.get_mut(&take_id).unwrap().media_path = PathBuf::from("../outside.mp4");
        assert!(matches!(
            plan("BLD-test", BuildKind::Draft, &state, &bundle),
            Err(BuildError::Path(_))
        ));
    }

    #[test]
    fn capability_listing_handles_combined_format_names() {
        assert!(listing_contains(" D  mov,mp4,m4a,3gp", "mp4"));
        assert!(!listing_contains(" V..... h264_videotoolbox", "libx264"));
    }

    #[test]
    fn review_report_serializes_recipe_lineage_and_contact_sheet() {
        let (state, bundle) = planning_fixture("audition");
        let recipe = plan("BLD-review", BuildKind::Draft, &state, &bundle).unwrap();
        let expected_input = recipe.inputs[0].clone();
        let report = BuildReviewReport {
            schema_version: BUILD_REVIEW_REPORT_SCHEMA_VERSION.to_owned(),
            build_id: recipe.build_id.clone(),
            project_id: recipe.project_id.clone(),
            kind: recipe.kind,
            recipe_path: PathBuf::from("builds/BLD-review/recipe.json"),
            output_path: recipe.output_path.clone(),
            delivery_path: recipe.delivery_path.clone(),
            contact_sheet_path: PathBuf::from("builds/BLD-review/contact-sheet.jpg"),
            recipe,
            media: MediaReport {
                valid: true,
                duration_seconds: 5.0,
                fps: 24.0,
                width: 960,
                height: 544,
                audio_channels: Some(2),
                checks: Vec::new(),
            },
            human_visual_review_required: true,
        };

        let value = serde_json::to_value(report).unwrap();
        assert_eq!(
            value["contact_sheet_path"],
            "builds/BLD-review/contact-sheet.jpg"
        );
        assert_eq!(
            value["recipe"]["inputs"][0]["take_id"],
            expected_input.take_id
        );
        assert_eq!(
            value["recipe"]["inputs"][0]["input_hash"],
            expected_input.input_hash
        );
        assert_eq!(
            value["recipe"]["inputs"][0]["workflow_hash"],
            expected_input.workflow_hash
        );
        assert_eq!(value["recipe"]["inputs"][0]["seed"], expected_input.seed);
        assert_eq!(value["human_visual_review_required"], true);
    }

    #[test]
    fn executor_reports_started_before_terminal_event() {
        let directory = tempfile::tempdir().unwrap();
        let (state, bundle) = planning_fixture("audition");
        let mut recipe = plan("BLD-events", BuildKind::Draft, &state, &bundle).unwrap();
        recipe.inputs.clear();
        let executor = BuildExecutorHandle::spawn().unwrap();
        executor
            .send(BuildRequest {
                project_id: state.project_id,
                project_root: directory.path().to_owned(),
                command_id: "build-command".to_owned(),
                recipe,
            })
            .unwrap();

        let receive = || {
            for _ in 0..100 {
                match executor.try_recv() {
                    Ok(event) => return event,
                    Err(TryRecvError::Empty) => {
                        std::thread::sleep(std::time::Duration::from_millis(5));
                    }
                    Err(TryRecvError::Disconnected) => panic!("build executor disconnected"),
                }
            }
            panic!("build executor event timed out")
        };

        assert!(matches!(receive(), BuildEvent::Started(_)));
        assert!(matches!(receive(), BuildEvent::Failed { .. }));
    }
}
