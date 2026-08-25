use std::ffi::OsStr;
use std::process::{Command, Output};

pub(crate) fn ffmpeg_fixture_runtime_available(required_filters: &[&str]) -> bool {
    if !command_succeeds("ffmpeg", ["-version"]) || !command_succeeds("ffprobe", ["-version"]) {
        return false;
    }
    let Ok(output) = Command::new("ffmpeg")
        .args(["-hide_banner", "-filters"])
        .output()
    else {
        return false;
    };
    if !output.status.success() {
        return false;
    }
    let listing = combined_output(&output);
    required_filters
        .iter()
        .all(|expected| listing_has_token(&listing, expected))
}

pub(crate) fn run_ffmpeg<I, S>(arguments: I)
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let output = Command::new("ffmpeg")
        .args(arguments)
        .output()
        .expect("fixture preflight succeeded but ffmpeg could not start");
    assert!(
        output.status.success(),
        "ffmpeg fixture generation failed:\n{}",
        combined_output(&output)
    );
}

fn command_succeeds<I, S>(program: &str, arguments: I) -> bool
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    Command::new(program)
        .args(arguments)
        .output()
        .is_ok_and(|output| output.status.success())
}

fn combined_output(output: &Output) -> String {
    let mut value = String::from_utf8_lossy(&output.stdout).into_owned();
    value.push_str(&String::from_utf8_lossy(&output.stderr));
    value
}

fn listing_has_token(listing: &str, expected: &str) -> bool {
    listing
        .lines()
        .flat_map(str::split_whitespace)
        .any(|token| token.trim_end_matches(',') == expected)
}
