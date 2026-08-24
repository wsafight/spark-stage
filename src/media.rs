use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaCheckStatus {
    Pass,
    Fail,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MediaCheck {
    pub code: String,
    pub status: MediaCheckStatus,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MediaReport {
    pub valid: bool,
    pub duration_seconds: f64,
    pub fps: f64,
    pub width: u32,
    pub height: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_channels: Option<u32>,
    pub checks: Vec<MediaCheck>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoundaryFrames {
    pub first: PathBuf,
    pub last: PathBuf,
    pub handoff_candidate: PathBuf,
}

pub fn inspect(
    path: &Path,
    expected_duration_seconds: u32,
    require_audio: bool,
) -> Result<MediaReport, MediaError> {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration:stream=codec_type,width,height,r_frame_rate,channels",
            "-of",
            "json",
        ])
        .arg(path)
        .output()
        .map_err(|source| MediaError::Command {
            program: "ffprobe",
            source,
        })?;
    if !output.status.success() {
        return Err(MediaError::ProbeFailed(first_line(&output.stderr)));
    }
    let probe: ProbeOutput = serde_json::from_slice(&output.stdout)?;
    let duration = probe
        .format
        .duration
        .as_deref()
        .and_then(|value| value.parse::<f64>().ok())
        .filter(|value| value.is_finite() && *value > 0.0)
        .ok_or_else(|| MediaError::InvalidProbe("positive duration is missing".to_owned()))?;
    let video = probe
        .streams
        .iter()
        .find(|stream| stream.codec_type.as_deref() == Some("video"))
        .ok_or_else(|| MediaError::InvalidProbe("video stream is missing".to_owned()))?;
    let width = video
        .width
        .filter(|value| *value > 0)
        .ok_or_else(|| MediaError::InvalidProbe("video width is missing".to_owned()))?;
    let height = video
        .height
        .filter(|value| *value > 0)
        .ok_or_else(|| MediaError::InvalidProbe("video height is missing".to_owned()))?;
    let fps = parse_rate(video.r_frame_rate.as_deref().unwrap_or_default())
        .filter(|value| value.is_finite() && *value > 0.0)
        .ok_or_else(|| MediaError::InvalidProbe("video frame rate is missing".to_owned()))?;
    let audio_channels = probe
        .streams
        .iter()
        .find(|stream| stream.codec_type.as_deref() == Some("audio"))
        .and_then(|stream| stream.channels);

    let tolerance = (f64::from(expected_duration_seconds) * 0.1).max(0.5);
    let duration_ok = (duration - f64::from(expected_duration_seconds)).abs() <= tolerance;
    let audio_ok = !require_audio || audio_channels.is_some_and(|channels| channels > 0);
    let black = filter_metric(
        path,
        &["-vf", "blackdetect=d=0.5:pix_th=0.10", "-an"],
        "black_duration:",
    )?;
    let freeze = filter_metric(
        path,
        &["-vf", "freezedetect=n=-50dB:d=1.5", "-an"],
        "freeze_duration:",
    )?;
    let silence = if audio_channels.is_some() {
        Some(filter_metric(
            path,
            &["-af", "silencedetect=n=-50dB:d=1.0", "-vn"],
            "silence_duration:",
        )?)
    } else {
        None
    };

    let black_ok = black < duration * 0.8;
    let freeze_ok = freeze < duration * 0.95;
    let silence_ok = silence.is_none_or(|seconds| seconds < duration * 0.9);
    let checks = vec![
        check(
            "DECODE_OK",
            true,
            format!("video {width}x{height} at {fps:.3} fps"),
        ),
        check(
            "DURATION_OK",
            duration_ok,
            format!(
                "actual {duration:.3}s; expected {expected_duration_seconds}s +/- {tolerance:.3}s"
            ),
        ),
        check(
            "AUDIO_PRESENT",
            audio_ok,
            audio_channels.map_or_else(
                || "audio stream is missing".to_owned(),
                |channels| format!("{channels} audio channels"),
            ),
        ),
        check(
            "BLACK_FRAME_LIMIT",
            black_ok,
            format!("{black:.3}s detected as black"),
        ),
        check(
            "FREEZE_LIMIT",
            freeze_ok,
            format!("{freeze:.3}s detected as frozen"),
        ),
        check(
            "SILENCE_LIMIT",
            silence_ok,
            silence.map_or_else(
                || "not evaluated without an audio stream".to_owned(),
                |seconds| format!("{seconds:.3}s detected as silence"),
            ),
        ),
    ];
    Ok(MediaReport {
        valid: checks
            .iter()
            .all(|check| check.status == MediaCheckStatus::Pass),
        duration_seconds: duration,
        fps,
        width,
        height,
        audio_channels,
        checks,
    })
}

pub fn missing_runtime_capabilities() -> Result<Vec<String>, MediaError> {
    let filters = command_listing("-filters")?;
    let muxers = command_listing("-muxers")?;
    let mut missing = Vec::new();
    for name in ["blackdetect", "freezedetect", "silencedetect"] {
        if !listing_contains(&filters, name) {
            missing.push(format!("filter:{name}"));
        }
    }
    for name in ["null", "image2"] {
        if !listing_contains(&muxers, name) {
            missing.push(format!("muxer:{name}"));
        }
    }
    Ok(missing)
}

pub fn extract_boundaries(
    media: &Path,
    review_dir: &Path,
    take_id: &str,
    duration_seconds: f64,
) -> Result<BoundaryFrames, MediaError> {
    std::fs::create_dir_all(review_dir).map_err(|source| MediaError::Io {
        path: review_dir.to_owned(),
        source,
    })?;
    let frames = BoundaryFrames {
        first: review_dir.join(format!("{take_id}-first.jpg")),
        last: review_dir.join(format!("{take_id}-last.jpg")),
        handoff_candidate: review_dir.join(format!("{take_id}-handoff-candidate.jpg")),
    };
    extract_frame(media, &frames.first, 0.05_f64.min(duration_seconds / 2.0))?;
    extract_frame(media, &frames.last, (duration_seconds - 0.05).max(0.0))?;
    extract_frame(
        media,
        &frames.handoff_candidate,
        (duration_seconds - 0.15).max(0.0),
    )?;
    Ok(frames)
}

fn extract_frame(media: &Path, output: &Path, seconds: f64) -> Result<(), MediaError> {
    let result = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-nostdin",
            "-y",
            "-ss",
        ])
        .arg(format!("{seconds:.3}"))
        .arg("-i")
        .arg(media)
        .args(["-frames:v", "1", "-q:v", "2"])
        .arg(output)
        .output()
        .map_err(|source| MediaError::Command {
            program: "ffmpeg",
            source,
        })?;
    if result.status.success() && output.is_file() {
        Ok(())
    } else {
        Err(MediaError::FilterFailed(last_line(&result.stderr)))
    }
}

fn filter_metric(path: &Path, filter_args: &[&str], marker: &str) -> Result<f64, MediaError> {
    let output = Command::new("ffmpeg")
        .args(["-hide_banner", "-nostdin", "-i"])
        .arg(path)
        .args(filter_args)
        .args(["-f", "null", "-"])
        .output()
        .map_err(|source| MediaError::Command {
            program: "ffmpeg",
            source,
        })?;
    if !output.status.success() {
        return Err(MediaError::FilterFailed(last_line(&output.stderr)));
    }
    Ok(sum_metric(&String::from_utf8_lossy(&output.stderr), marker))
}

fn command_listing(argument: &str) -> Result<String, MediaError> {
    let output = Command::new("ffmpeg")
        .args(["-hide_banner", argument])
        .output()
        .map_err(|source| MediaError::Command {
            program: "ffmpeg",
            source,
        })?;
    if !output.status.success() {
        return Err(MediaError::FilterFailed(last_line(&output.stderr)));
    }
    let mut listing = String::from_utf8_lossy(&output.stdout).into_owned();
    listing.push_str(&String::from_utf8_lossy(&output.stderr));
    Ok(listing)
}

fn listing_contains(listing: &str, expected: &str) -> bool {
    listing
        .lines()
        .flat_map(str::split_whitespace)
        .any(|token| token == expected)
}

fn check(code: &str, passed: bool, detail: String) -> MediaCheck {
    MediaCheck {
        code: code.to_owned(),
        status: if passed {
            MediaCheckStatus::Pass
        } else {
            MediaCheckStatus::Fail
        },
        detail,
    }
}

fn parse_rate(value: &str) -> Option<f64> {
    let (numerator, denominator) = value.split_once('/')?;
    let numerator = numerator.parse::<f64>().ok()?;
    let denominator = denominator.parse::<f64>().ok()?;
    (denominator != 0.0).then_some(numerator / denominator)
}

fn sum_metric(source: &str, marker: &str) -> f64 {
    source
        .lines()
        .flat_map(str::split_whitespace)
        .filter_map(|field| field.strip_prefix(marker))
        .filter_map(|value| value.parse::<f64>().ok())
        .sum()
}

fn first_line(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("command failed without diagnostics")
        .trim()
        .to_owned()
}

fn last_line(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("command failed without diagnostics")
        .trim()
        .to_owned()
}

#[derive(Debug, Deserialize)]
struct ProbeOutput {
    #[serde(default)]
    streams: Vec<ProbeStream>,
    #[serde(default)]
    format: ProbeFormat,
}

#[derive(Debug, Deserialize)]
struct ProbeStream {
    codec_type: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    r_frame_rate: Option<String>,
    channels: Option<u32>,
}

#[derive(Debug, Default, Deserialize)]
struct ProbeFormat {
    duration: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum MediaError {
    #[error("cannot run {program}: {source}")]
    Command {
        program: &'static str,
        source: std::io::Error,
    },
    #[error("ffprobe failed: {0}")]
    ProbeFailed(String),
    #[error("ffprobe output is invalid: {0}")]
    InvalidProbe(String),
    #[error("cannot decode ffprobe JSON: {0}")]
    Decode(#[from] serde_json::Error),
    #[error("ffmpeg media check failed: {0}")]
    FilterFailed(String),
    #[error("cannot access `{path}`: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fractional_frame_rate() {
        assert_eq!(parse_rate("30000/1001"), Some(30000.0 / 1001.0));
        assert_eq!(parse_rate("24/0"), None);
        assert_eq!(parse_rate("invalid"), None);
    }

    #[test]
    fn sums_repeated_detector_durations() {
        let log = "black_start:0 black_duration:1.25\nblack_start:4 black_duration:0.75";
        assert_eq!(sum_metric(log, "black_duration:"), 2.0);
    }

    #[test]
    fn reference_video_passes_probe_and_extracts_boundaries() {
        if Command::new("ffmpeg").arg("-version").output().is_err()
            || Command::new("ffprobe").arg("-version").output().is_err()
        {
            return;
        }
        if !matches!(missing_runtime_capabilities(), Ok(missing) if missing.is_empty()) {
            return;
        }
        let media = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("reference/OpenMontage/assets/signal-from-tomorrow-demo.mp4");
        if !media.is_file() {
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let report = inspect(&media, 30, true).unwrap();
        assert!(report.valid, "{report:?}");
        assert_eq!(report.audio_channels, Some(2));
        let boundaries = extract_boundaries(
            &media,
            &directory.path().join("review"),
            "TAKE-TEST",
            report.duration_seconds,
        )
        .unwrap();
        assert!(boundaries.first.is_file());
        assert!(boundaries.last.is_file());
        assert!(boundaries.handoff_candidate.is_file());
    }
}
