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

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MediaCheckPolicy {
    #[serde(default = "default_max_freeze_ratio")]
    pub max_freeze_ratio: f64,
}

impl Default for MediaCheckPolicy {
    fn default() -> Self {
        Self {
            max_freeze_ratio: default_max_freeze_ratio(),
        }
    }
}

impl MediaCheckPolicy {
    pub fn validate(self) -> Result<(), String> {
        if self.max_freeze_ratio.is_finite() && (0.0..=1.0).contains(&self.max_freeze_ratio) {
            Ok(())
        } else {
            Err("max_freeze_ratio must be finite and between 0.0 and 1.0".to_owned())
        }
    }
}

const fn default_max_freeze_ratio() -> f64 {
    0.30
}

pub fn inspect(
    path: &Path,
    expected_duration_seconds: u32,
    require_audio: bool,
) -> Result<MediaReport, MediaError> {
    inspect_with_policy(
        path,
        expected_duration_seconds,
        require_audio,
        MediaCheckPolicy::default(),
    )
}

pub fn inspect_with_policy(
    path: &Path,
    expected_duration_seconds: u32,
    require_audio: bool,
    policy: MediaCheckPolicy,
) -> Result<MediaReport, MediaError> {
    policy.validate().map_err(MediaError::InvalidPolicy)?;
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
    let freeze_log = filter_log(path, &["-vf", "freezedetect=n=-50dB:d=1.5", "-an"])?;
    let freeze =
        sum_metric_with_open_interval(&freeze_log, "freeze_duration:", "freeze_start:", duration);
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
    let freeze_ratio = freeze / duration;
    let freeze_ok = freeze_ratio <= policy.max_freeze_ratio;
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
            format!(
                "{freeze:.3}s detected as frozen ({:.1}% of media; limit {:.1}%)",
                freeze_ratio * 100.0,
                policy.max_freeze_ratio * 100.0
            ),
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
    let encoders = command_listing("-encoders")?;
    let mut missing = Vec::new();
    for name in [
        "blackdetect",
        "freezedetect",
        "silencedetect",
        "scale",
        "pad",
        "fps",
        "setsar",
        "aresample",
        "loudnorm",
        "concat",
        "trim",
        "setpts",
        "atrim",
        "asetpts",
    ] {
        if !listing_contains(&filters, name) {
            missing.push(format!("filter:{name}"));
        }
    }
    for name in ["null", "image2", "mp4"] {
        if !listing_contains(&muxers, name) {
            missing.push(format!("muxer:{name}"));
        }
    }
    for name in ["libx264", "aac"] {
        if !listing_contains(&encoders, name) {
            missing.push(format!("encoder:{name}"));
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
    extract_last_frame(media, &frames.last, duration_seconds)?;
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

fn extract_last_frame(
    media: &Path,
    output: &Path,
    duration_seconds: f64,
) -> Result<(), MediaError> {
    let lookback = duration_seconds.clamp(0.05, 1.0);
    let result = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-nostdin",
            "-y",
            "-sseof",
        ])
        .arg(format!("-{lookback:.3}"))
        .arg("-i")
        .arg(media)
        .args(["-map", "0:v:0", "-an", "-q:v", "2", "-update", "1"])
        .arg(output)
        .output()
        .map_err(|source| MediaError::Command {
            program: "ffmpeg",
            source,
        })?;
    if result.status.success() && output.is_file() {
        Ok(())
    } else if result.status.success() {
        Err(MediaError::FilterFailed(
            "ffmpeg produced no last frame".to_owned(),
        ))
    } else {
        Err(MediaError::FilterFailed(last_line(&result.stderr)))
    }
}

fn filter_metric(path: &Path, filter_args: &[&str], marker: &str) -> Result<f64, MediaError> {
    Ok(sum_metric(&filter_log(path, filter_args)?, marker))
}

fn filter_log(path: &Path, filter_args: &[&str]) -> Result<String, MediaError> {
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
    Ok(String::from_utf8_lossy(&output.stderr).into_owned())
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
    metric_values(source, marker).into_iter().sum()
}

fn sum_metric_with_open_interval(
    source: &str,
    duration_marker: &str,
    start_marker: &str,
    total_duration: f64,
) -> f64 {
    let durations = metric_values(source, duration_marker);
    let starts = metric_values(source, start_marker);
    let mut total = durations.iter().sum();
    if starts.len() > durations.len()
        && let Some(start) = starts.last()
    {
        total += (total_duration - start).max(0.0);
    }
    total
}

fn metric_values(source: &str, marker: &str) -> Vec<f64> {
    let fields = source.split_whitespace().collect::<Vec<_>>();
    fields
        .iter()
        .enumerate()
        .filter_map(|(index, field)| {
            let marker_position = field.rfind(marker)?;
            let attached = &field[marker_position + marker.len()..];
            let value = if attached.is_empty() {
                *fields.get(index + 1)?
            } else {
                attached
            };
            value.parse::<f64>().ok()
        })
        .collect()
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
    #[error("invalid media check policy: {0}")]
    InvalidPolicy(String),
    #[error("cannot access `{path}`: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;

    use super::*;
    use crate::test_support::{ffmpeg_fixture_runtime_available, run_ffmpeg};

    const SYNTHETIC_FILTERS: &[&str] = &["anullsrc", "color", "sine", "testsrc2"];

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
    fn parses_namespaced_metrics_and_counts_an_interval_open_at_eof() {
        let closed = "lavfi.freezedetect.freeze_start: 0.5 \
                      lavfi.freezedetect.freeze_duration: 1.25";
        assert_eq!(sum_metric(closed, "freeze_duration:"), 1.25);
        assert_eq!(
            sum_metric_with_open_interval(closed, "freeze_duration:", "freeze_start:", 2.0),
            1.25
        );

        let open = "[freezedetect] lavfi.freezedetect.freeze_start: 0";
        assert_eq!(
            sum_metric_with_open_interval(open, "freeze_duration:", "freeze_start:", 2.0),
            2.0
        );
    }

    #[test]
    fn rejects_invalid_media_check_policy() {
        assert!(
            MediaCheckPolicy {
                max_freeze_ratio: f64::NAN,
            }
            .validate()
            .is_err()
        );
        assert!(
            MediaCheckPolicy {
                max_freeze_ratio: 1.01,
            }
            .validate()
            .is_err()
        );
        assert!(
            MediaCheckPolicy {
                max_freeze_ratio: 0.0,
            }
            .validate()
            .is_ok()
        );
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

    #[test]
    fn synthetic_media_checks_audio_duration_and_frame_rate() {
        if !synthetic_runtime_available() {
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let with_audio = directory.path().join("with-audio.mp4");
        let without_audio = directory.path().join("without-audio.mp4");
        generate_video(
            &with_audio,
            "testsrc2=size=160x96:rate=12:duration=2",
            Some("sine=frequency=880:sample_rate=48000:duration=2"),
        );
        generate_video(
            &without_audio,
            "testsrc2=size=160x96:rate=12:duration=2",
            None,
        );

        let report = inspect(&with_audio, 2, true).unwrap();
        assert!(report.valid, "{report:#?}");
        assert!((report.fps - 12.0).abs() < 0.01, "{report:#?}");
        assert_eq!((report.width, report.height), (160, 96));
        assert!(report.audio_channels.is_some());

        let wrong_duration = inspect(&with_audio, 5, true).unwrap();
        assert_check(&wrong_duration, "DURATION_OK", MediaCheckStatus::Fail);
        assert!(!wrong_duration.valid);

        let required = inspect(&without_audio, 2, true).unwrap();
        assert_check(&required, "AUDIO_PRESENT", MediaCheckStatus::Fail);
        assert!(!required.valid);
        let optional = inspect(&without_audio, 2, false).unwrap();
        assert_check(&optional, "AUDIO_PRESENT", MediaCheckStatus::Pass);
        assert!(optional.valid, "{optional:#?}");
    }

    #[test]
    fn synthetic_media_detects_black_freeze_and_silence() {
        if !synthetic_runtime_available() {
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let black = directory.path().join("black.mp4");
        let static_frame = directory.path().join("static.mp4");
        let silent = directory.path().join("silent.mp4");
        generate_video(
            &black,
            "color=c=black:s=160x96:r=12:d=2",
            Some("sine=frequency=880:sample_rate=48000:duration=2"),
        );
        generate_video(
            &static_frame,
            "color=c=red:s=160x96:r=12:d=2",
            Some("sine=frequency=880:sample_rate=48000:duration=2"),
        );
        generate_video(
            &silent,
            "testsrc2=size=160x96:rate=12:duration=2",
            Some("anullsrc=r=48000:cl=stereo"),
        );

        let black_report = inspect(&black, 2, true).unwrap();
        assert_check(&black_report, "BLACK_FRAME_LIMIT", MediaCheckStatus::Fail);
        let static_report = inspect(&static_frame, 2, true).unwrap();
        assert_check(&static_report, "FREEZE_LIMIT", MediaCheckStatus::Fail);
        let silent_report = inspect(&silent, 2, true).unwrap();
        assert_check(&silent_report, "SILENCE_LIMIT", MediaCheckStatus::Fail);
    }

    #[test]
    fn profile_freeze_limit_rejects_observed_short_video_regression() {
        if !synthetic_runtime_available() {
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let media = directory.path().join("partial-freeze.mp4");
        generate_video(
            &media,
            "testsrc2=size=160x96:rate=24:duration=3.292[move];\
             color=c=red:s=160x96:r=24:d=1.875[still];\
             [move][still]concat=n=2:v=1:a=0",
            None,
        );

        let strict = inspect_with_policy(
            &media,
            5,
            false,
            MediaCheckPolicy {
                max_freeze_ratio: 0.30,
            },
        )
        .unwrap();
        assert_check(&strict, "FREEZE_LIMIT", MediaCheckStatus::Fail);
        assert!(!strict.valid, "{strict:#?}");

        let lenient = inspect_with_policy(
            &media,
            5,
            false,
            MediaCheckPolicy {
                max_freeze_ratio: 0.40,
            },
        )
        .unwrap();
        assert_check(&lenient, "FREEZE_LIMIT", MediaCheckStatus::Pass);
        assert!(lenient.valid, "{lenient:#?}");
    }

    #[test]
    fn synthetic_media_extracts_distinct_boundary_frames() {
        if !synthetic_runtime_available() {
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let media = directory.path().join("moving.mp4");
        generate_video(
            &media,
            "testsrc2=size=160x96:rate=12:duration=2",
            Some("sine=frequency=880:sample_rate=48000:duration=2"),
        );
        let frames = extract_boundaries(
            &media,
            &directory.path().join("review"),
            "TAKE-SYNTHETIC",
            2.0,
        )
        .unwrap();

        for path in [&frames.first, &frames.last, &frames.handoff_candidate] {
            assert!(path.is_file());
            assert!(std::fs::metadata(path).unwrap().len() > 0);
        }
        assert_ne!(
            std::fs::read(&frames.first).unwrap(),
            std::fs::read(&frames.last).unwrap()
        );
    }

    fn synthetic_runtime_available() -> bool {
        ffmpeg_fixture_runtime_available(SYNTHETIC_FILTERS)
            && matches!(missing_runtime_capabilities(), Ok(missing) if missing.is_empty())
    }

    fn generate_video(path: &Path, video_source: &str, audio_source: Option<&str>) {
        let mut arguments = vec![
            "-hide_banner",
            "-loglevel",
            "error",
            "-nostdin",
            "-y",
            "-f",
            "lavfi",
            "-i",
            video_source,
        ];
        if let Some(source) = audio_source {
            arguments.extend(["-f", "lavfi", "-i", source, "-t", "2", "-shortest"]);
        }
        arguments.extend(["-c:v", "libx264", "-pix_fmt", "yuv420p"]);
        if audio_source.is_some() {
            arguments.extend(["-c:a", "aac"]);
        }
        run_ffmpeg(
            arguments
                .into_iter()
                .map(OsStr::new)
                .chain([path.as_os_str()]),
        );
    }

    fn assert_check(report: &MediaReport, code: &str, expected: MediaCheckStatus) {
        let check = report
            .checks
            .iter()
            .find(|check| check.code == code)
            .unwrap_or_else(|| panic!("missing media check {code}: {report:#?}"));
        assert_eq!(check.status, expected, "{check:#?}");
    }
}
