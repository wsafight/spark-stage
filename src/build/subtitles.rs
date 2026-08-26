use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{BuildError, BuildKind};
use crate::domain::{DialogueLine, ScriptBundle};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubtitleCue {
    pub shot_id: String,
    pub start_milliseconds: u64,
    pub end_milliseconds: u64,
    pub speaker: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubtitleTrack {
    pub source_hash: String,
    pub cues: Vec<SubtitleCue>,
    pub srt_path: PathBuf,
    pub vtt_path: PathBuf,
    pub delivery_srt_path: PathBuf,
    pub delivery_vtt_path: PathBuf,
    pub srt_sha256: String,
    pub vtt_sha256: String,
}

pub(super) fn plan(
    build_id: &str,
    project_id: &str,
    kind: BuildKind,
    bundle: &ScriptBundle,
    selected_shot_ids: &[String],
) -> Result<Option<SubtitleTrack>, BuildError> {
    let mut offset = 0_u64;
    let mut cues = Vec::new();
    let mut source = Vec::new();
    for shot in &bundle.shots {
        if !selected_shot_ids.is_empty()
            && !selected_shot_ids.iter().any(|shot_id| shot_id == &shot.id)
        {
            continue;
        }
        let duration_seconds = if kind == BuildKind::Trailer {
            2_u32.min(shot.duration)
        } else {
            shot.duration
        };
        let duration_milliseconds = u64::from(duration_seconds) * 1_000;
        source.push((&shot.id, duration_seconds, &shot.dialogue));
        append_shot_cues(
            &mut cues,
            &shot.id,
            offset,
            duration_milliseconds,
            &shot.dialogue,
        );
        offset = offset
            .checked_add(duration_milliseconds)
            .ok_or(BuildError::DurationOverflow)?;
    }
    if cues.is_empty() {
        return Ok(None);
    }
    let source_hash = crate::store::sha256_json(&source)
        .map_err(|error| BuildError::Subtitle(error.to_string()))?;
    let srt = render_srt(&cues);
    let vtt = render_vtt(&cues);
    let base = PathBuf::from("builds").join(build_id);
    let delivery_base = match kind {
        BuildKind::Draft => PathBuf::from("review/draft-cut"),
        BuildKind::Trailer => PathBuf::from("final").join(format!("{project_id}-trailer")),
        BuildKind::Final => PathBuf::from("final").join(project_id),
    };
    Ok(Some(SubtitleTrack {
        source_hash,
        cues,
        srt_path: base.join("subtitles.srt"),
        vtt_path: base.join("subtitles.vtt"),
        delivery_srt_path: delivery_base.with_extension("srt"),
        delivery_vtt_path: delivery_base.with_extension("vtt"),
        srt_sha256: crate::store::sha256_bytes(srt.as_bytes()),
        vtt_sha256: crate::store::sha256_bytes(vtt.as_bytes()),
    }))
}

pub(super) fn write(project_root: &Path, track: &SubtitleTrack) -> Result<(), BuildError> {
    let srt = render_srt(&track.cues);
    let vtt = render_vtt(&track.cues);
    if crate::store::sha256_bytes(srt.as_bytes()) != track.srt_sha256
        || crate::store::sha256_bytes(vtt.as_bytes()) != track.vtt_sha256
    {
        return Err(BuildError::Subtitle(
            "subtitle cue content does not match the frozen hashes".to_owned(),
        ));
    }
    for (relative, content) in [(&track.srt_path, &srt), (&track.vtt_path, &vtt)] {
        let path = super::project_path(project_root, relative)?;
        crate::store::write_text_atomic(&path, content)
            .map_err(|error| BuildError::Subtitle(error.to_string()))?;
    }
    super::publish_copy(
        &super::project_path(project_root, &track.srt_path)?,
        &super::project_path(project_root, &track.delivery_srt_path)?,
    )?;
    super::publish_copy(
        &super::project_path(project_root, &track.vtt_path)?,
        &super::project_path(project_root, &track.delivery_vtt_path)?,
    )
}

pub(super) fn remove_delivery(
    project_root: &Path,
    delivery_video: &Path,
) -> Result<(), BuildError> {
    for relative in [
        delivery_video.with_extension("srt"),
        delivery_video.with_extension("vtt"),
    ] {
        let path = super::project_path(project_root, &relative)?;
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(BuildError::Subtitle(error.to_string())),
        }
    }
    Ok(())
}

fn append_shot_cues(
    cues: &mut Vec<SubtitleCue>,
    shot_id: &str,
    offset: u64,
    duration: u64,
    dialogue: &[DialogueLine],
) {
    let Ok(count) = u64::try_from(dialogue.len()) else {
        return;
    };
    if count == 0 || duration == 0 {
        return;
    }
    for (index, line) in dialogue.iter().enumerate() {
        let index = u64::try_from(index).unwrap_or(0);
        let start = offset + duration.saturating_mul(index) / count;
        let end = offset + duration.saturating_mul(index + 1) / count;
        cues.push(SubtitleCue {
            shot_id: shot_id.to_owned(),
            start_milliseconds: start,
            end_milliseconds: end,
            speaker: normalize(&line.who),
            text: normalize(&line.text),
        });
    }
}

fn render_srt(cues: &[SubtitleCue]) -> String {
    let mut output = String::new();
    for (index, cue) in cues.iter().enumerate() {
        output.push_str(&format!(
            "{}\n{} --> {}\n{}: {}\n\n",
            index + 1,
            timestamp(cue.start_milliseconds, ','),
            timestamp(cue.end_milliseconds, ','),
            cue.speaker,
            cue.text
        ));
    }
    output
}

fn render_vtt(cues: &[SubtitleCue]) -> String {
    let mut output = String::from("WEBVTT\n\n");
    for cue in cues {
        output.push_str(&format!(
            "{}\n{} --> {}\n{}: {}\n\n",
            cue.shot_id,
            timestamp(cue.start_milliseconds, '.'),
            timestamp(cue.end_milliseconds, '.'),
            cue.speaker,
            cue.text
        ));
    }
    output
}

fn timestamp(milliseconds: u64, separator: char) -> String {
    let hours = milliseconds / 3_600_000;
    let minutes = (milliseconds / 60_000) % 60;
    let seconds = (milliseconds / 1_000) % 60;
    let millis = milliseconds % 1_000;
    format!("{hours:02}:{minutes:02}:{seconds:02}{separator}{millis:03}")
}

fn normalize(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    const BUNDLE: &str = include_str!("../../skills/screenwriter/examples/valid-short-drama.json");

    #[test]
    fn srt_and_vtt_use_deterministic_shot_offsets() {
        let bundle: ScriptBundle = serde_json::from_str(BUNDLE).unwrap();
        let track = plan("BLD-test", "demo", BuildKind::Final, &bundle, &[])
            .unwrap()
            .unwrap();

        let srt = render_srt(&track.cues);
        let vtt = render_vtt(&track.cues);
        assert!(srt.contains("00:00:00,000 -->"));
        assert!(vtt.starts_with("WEBVTT\n\nS01\n00:00:00.000 -->"));
        assert_eq!(crate::store::sha256_bytes(srt.as_bytes()), track.srt_sha256);
        assert_eq!(crate::store::sha256_bytes(vtt.as_bytes()), track.vtt_sha256);
    }

    #[test]
    fn scoped_draft_uses_only_selected_shot_dialogue() {
        let bundle: ScriptBundle = serde_json::from_str(BUNDLE).unwrap();
        let track = plan(
            "BLD-test",
            "demo",
            BuildKind::Draft,
            &bundle,
            &["S02".to_owned()],
        )
        .unwrap()
        .unwrap();

        assert!(track.cues.iter().all(|cue| cue.shot_id == "S02"));
        assert_eq!(track.cues[0].start_milliseconds, 0);
    }

    #[test]
    fn dialogue_free_build_removes_stale_delivery_subtitles() {
        let directory = tempfile::tempdir().unwrap();
        let delivery = Path::new("final/demo.mp4");
        let final_dir = directory.path().join("final");
        std::fs::create_dir_all(&final_dir).unwrap();
        std::fs::write(final_dir.join("demo.srt"), "stale").unwrap();
        std::fs::write(final_dir.join("demo.vtt"), "stale").unwrap();

        remove_delivery(directory.path(), delivery).unwrap();

        assert!(!final_dir.join("demo.srt").exists());
        assert!(!final_dir.join("demo.vtt").exists());
    }
}
