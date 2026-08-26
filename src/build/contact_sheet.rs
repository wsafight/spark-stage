use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use super::{BuildError, BuildRecipe, project_path, publish_copy};

const CELL_WIDTH: usize = 320;
const CELL_HEIGHT: usize = 180;
const MAX_COLUMNS: usize = 4;

pub(super) fn create(project_root: &Path, recipe: &BuildRecipe) -> Result<PathBuf, BuildError> {
    let frames = frame_paths(recipe)?;
    let relative = PathBuf::from("builds")
        .join(&recipe.build_id)
        .join("contact-sheet.jpg");
    let output = project_path(project_root, &relative)?;
    let temporary = output.with_file_name(".contact-sheet.tmp.jpg");
    if temporary.exists() {
        fs::remove_file(&temporary).map_err(|error| {
            BuildError::ContactSheet(format!("cannot remove staging contact sheet: {error}"))
        })?;
    }

    let mut command = Command::new("ffmpeg");
    command.args(["-hide_banner", "-loglevel", "error", "-nostdin", "-y"]);
    for frame in frames {
        command.arg("-i").arg(project_path(project_root, frame)?);
    }
    command
        .arg("-filter_complex")
        .arg(filter_graph(recipe.inputs.len()))
        .args(["-map", "[sheet]", "-frames:v", "1", "-q:v", "2"])
        .arg(&temporary)
        .stdout(Stdio::null());
    let result = command
        .output()
        .map_err(|error| BuildError::ContactSheet(format!("cannot start ffmpeg: {error}")))?;
    if !result.status.success() || !temporary.is_file() {
        return Err(BuildError::ContactSheet(
            String::from_utf8_lossy(&result.stderr)
                .lines()
                .rev()
                .find(|line| !line.trim().is_empty())
                .unwrap_or("ffmpeg did not create a contact sheet")
                .trim()
                .to_owned(),
        ));
    }
    fs::rename(&temporary, &output).map_err(|error| {
        BuildError::ContactSheet(format!("cannot publish build contact sheet: {error}"))
    })?;
    let review_copy = project_root.join("review/contact-sheet.jpg");
    publish_copy(&output, &review_copy)?;
    Ok(relative)
}

fn frame_paths(recipe: &BuildRecipe) -> Result<Vec<&Path>, BuildError> {
    recipe
        .inputs
        .iter()
        .map(|input| {
            input.first_frame_path.as_deref().ok_or_else(|| {
                BuildError::ContactSheet(format!(
                    "take `{}` has no extracted first frame",
                    input.take_id
                ))
            })
        })
        .collect()
}

fn filter_graph(input_count: usize) -> String {
    let columns = input_count.min(MAX_COLUMNS);
    let mut filters = Vec::with_capacity(input_count + 1);
    let mut labels = String::new();
    let mut layout = Vec::with_capacity(input_count);
    for index in 0..input_count {
        filters.push(format!(
            "[{index}:v]scale={CELL_WIDTH}:{CELL_HEIGHT}:force_original_aspect_ratio=decrease,pad={CELL_WIDTH}:{CELL_HEIGHT}:(ow-iw)/2:(oh-ih)/2[v{index}]"
        ));
        labels.push_str(&format!("[v{index}]"));
        let column = index % columns;
        let row = index / columns;
        layout.push(format!("{}_{}", column * CELL_WIDTH, row * CELL_HEIGHT));
    }
    if input_count == 1 {
        filters.push("[v0]null[sheet]".to_owned());
    } else {
        filters.push(format!(
            "{labels}xstack=inputs={input_count}:layout={}[sheet]",
            layout.join("|")
        ));
    }
    filters.join(";")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::{BUILD_RECIPE_SCHEMA_VERSION, BuildInput, BuildKind};

    fn recipe_with_first_frame(first_frame_path: Option<PathBuf>) -> BuildRecipe {
        BuildRecipe {
            schema_version: BUILD_RECIPE_SCHEMA_VERSION.to_owned(),
            build_id: "BLD-contact".to_owned(),
            project_id: "demo".to_owned(),
            contract_id: Some("CONTRACT-1".to_owned()),
            contract_hash: "contract-hash".to_owned(),
            source_revision: 7,
            kind: BuildKind::Draft,
            width: 960,
            height: 544,
            fps: 24,
            expected_duration_seconds: 5,
            inputs: vec![BuildInput {
                shot_id: "S01".to_owned(),
                take_id: "TAKE-1".to_owned(),
                media_path: PathBuf::from("raw/S01/TAKE-1.mp4"),
                profile: "audition".to_owned(),
                input_hash: "input-hash".to_owned(),
                adapter_fingerprint: "adapter".to_owned(),
                workflow_hash: "workflow".to_owned(),
                model_fingerprint: "model".to_owned(),
                seed: 42,
                reference_subjects: Vec::new(),
                reference_fingerprint: String::new(),
                warnings: Vec::new(),
                first_frame_path,
                trim_seconds: None,
            }],
            subtitles: None,
            output_path: PathBuf::from("builds/BLD-contact/output.mp4"),
            delivery_path: PathBuf::from("review/draft-cut.mp4"),
        }
    }

    #[test]
    fn contact_sheet_layout_is_stable_for_multiple_rows() {
        let graph = filter_graph(5);

        assert!(graph.contains("xstack=inputs=5"));
        assert!(graph.contains("layout=0_0|320_0|640_0|960_0|0_180"));
        assert_eq!(graph.matches("scale=320:180").count(), 5);
    }

    #[test]
    fn single_frame_contact_sheet_does_not_require_xstack() {
        let graph = filter_graph(1);

        assert!(graph.contains("[v0]null[sheet]"));
        assert!(!graph.contains("xstack"));
    }

    #[test]
    fn missing_first_frame_fails_before_ffmpeg_is_started() {
        let error = frame_paths(&recipe_with_first_frame(None)).unwrap_err();

        assert_eq!(
            error.to_string(),
            "cannot create contact sheet: take `TAKE-1` has no extracted first frame"
        );
    }
}
