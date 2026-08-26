use std::collections::HashSet;
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Sender};

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const NOTIFICATION_SCHEMA_VERSION: &str = "1.0";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MilestoneKind {
    ApprovalRequired,
    TakeReady,
    CameraFailed,
    BuildCompleted,
    BuildFailed,
    DiskBlocked,
    ProjectCompleted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HookConfig {
    pub schema_version: String,
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executable: Option<PathBuf>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default = "all_milestones")]
    pub events: Vec<MilestoneKind>,
}

impl Default for HookConfig {
    fn default() -> Self {
        Self {
            schema_version: NOTIFICATION_SCHEMA_VERSION.to_owned(),
            enabled: false,
            executable: None,
            args: Vec::new(),
            events: all_milestones(),
        }
    }
}

impl HookConfig {
    pub fn load(path: &Path) -> Result<Self, NotificationError> {
        let source = fs::read(path).map_err(|source| NotificationError::Io {
            path: path.to_owned(),
            source,
        })?;
        let config =
            serde_json::from_slice(&source).map_err(|source| NotificationError::Decode {
                path: path.to_owned(),
                source,
            })?;
        validate(&config)?;
        Ok(config)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MilestoneEvent {
    pub schema_version: String,
    pub kind: MilestoneKind,
    pub project_id: String,
    pub subject_id: String,
    pub message: String,
    pub occurred_at: String,
}

impl MilestoneEvent {
    #[must_use]
    pub fn new(
        kind: MilestoneKind,
        project_id: impl Into<String>,
        subject_id: impl Into<String>,
        message: impl Into<String>,
        occurred_at: impl Into<String>,
    ) -> Self {
        Self {
            schema_version: NOTIFICATION_SCHEMA_VERSION.to_owned(),
            kind,
            project_id: project_id.into(),
            subject_id: subject_id.into(),
            message: message.into(),
            occurred_at: occurred_at.into(),
        }
    }
}

pub struct HookDispatcher {
    sender: Sender<MilestoneEvent>,
}

impl HookDispatcher {
    pub fn from_config(config: HookConfig) -> Result<Option<Self>, NotificationError> {
        validate(&config)?;
        if !config.enabled {
            return Ok(None);
        }
        let executable = config
            .executable
            .clone()
            .ok_or(NotificationError::ExecutableRequired)?;
        let events = config.events.iter().copied().collect::<HashSet<_>>();
        let args = config.args;
        let (sender, receiver) = mpsc::channel::<MilestoneEvent>();
        std::thread::Builder::new()
            .name("sparkstage-hook".to_owned())
            .spawn(move || {
                while let Ok(event) = receiver.recv() {
                    if !events.contains(&event.kind) {
                        continue;
                    }
                    if let Err(error) = execute_hook(&executable, &args, &event) {
                        eprintln!("notification hook failed: {error}");
                    }
                }
            })
            .map_err(NotificationError::Thread)?;
        Ok(Some(Self { sender }))
    }

    pub fn emit(&self, event: MilestoneEvent) {
        if self.sender.send(event).is_err() {
            eprintln!("notification hook worker is unavailable");
        }
    }
}

pub fn validate(config: &HookConfig) -> Result<(), NotificationError> {
    if config.schema_version != NOTIFICATION_SCHEMA_VERSION {
        return Err(NotificationError::Schema(config.schema_version.clone()));
    }
    if config.args.len() > 32 || config.args.iter().any(|arg| arg.len() > 4_096) {
        return Err(NotificationError::Arguments);
    }
    let unique = config.events.iter().copied().collect::<HashSet<_>>();
    if config.events.is_empty() || unique.len() != config.events.len() {
        return Err(NotificationError::Events);
    }
    if !config.enabled {
        return Ok(());
    }
    let executable = config
        .executable
        .as_ref()
        .ok_or(NotificationError::ExecutableRequired)?;
    if !executable.is_absolute() {
        return Err(NotificationError::ExecutableAbsolute);
    }
    let metadata = fs::symlink_metadata(executable).map_err(|source| NotificationError::Io {
        path: executable.clone(),
        source,
    })?;
    if metadata.file_type().is_symlink()
        || !metadata.file_type().is_file()
        || metadata.permissions().mode() & 0o111 == 0
    {
        return Err(NotificationError::ExecutableRegular);
    }
    Ok(())
}

fn execute_hook(
    executable: &Path,
    args: &[String],
    event: &MilestoneEvent,
) -> Result<(), NotificationError> {
    let encoded = serde_json::to_vec(event).map_err(NotificationError::Encode)?;
    let mut child = Command::new(executable)
        .args(args)
        .env_clear()
        .env("SPARKSTAGE_EVENT", milestone_name(event.kind))
        .env("SPARKSTAGE_PROJECT_ID", &event.project_id)
        .env("SPARKSTAGE_SUBJECT_ID", &event.subject_id)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|source| NotificationError::Io {
            path: executable.to_owned(),
            source,
        })?;
    child
        .stdin
        .take()
        .ok_or(NotificationError::Stdin)?
        .write_all(&encoded)
        .map_err(|source| NotificationError::Io {
            path: executable.to_owned(),
            source,
        })?;
    let status = child.wait().map_err(|source| NotificationError::Io {
        path: executable.to_owned(),
        source,
    })?;
    if status.success() {
        Ok(())
    } else {
        Err(NotificationError::Exit(status.code()))
    }
}

const fn milestone_name(kind: MilestoneKind) -> &'static str {
    match kind {
        MilestoneKind::ApprovalRequired => "approval_required",
        MilestoneKind::TakeReady => "take_ready",
        MilestoneKind::CameraFailed => "camera_failed",
        MilestoneKind::BuildCompleted => "build_completed",
        MilestoneKind::BuildFailed => "build_failed",
        MilestoneKind::DiskBlocked => "disk_blocked",
        MilestoneKind::ProjectCompleted => "project_completed",
    }
}

fn all_milestones() -> Vec<MilestoneKind> {
    vec![
        MilestoneKind::ApprovalRequired,
        MilestoneKind::TakeReady,
        MilestoneKind::CameraFailed,
        MilestoneKind::BuildCompleted,
        MilestoneKind::BuildFailed,
        MilestoneKind::DiskBlocked,
        MilestoneKind::ProjectCompleted,
    ]
}

#[derive(Debug, Error)]
pub enum NotificationError {
    #[error("cannot access `{path}`: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("cannot decode hook config `{path}`: {source}")]
    Decode {
        path: PathBuf,
        source: serde_json::Error,
    },
    #[error("cannot encode milestone event: {0}")]
    Encode(serde_json::Error),
    #[error("unsupported notification schema `{0}`")]
    Schema(String),
    #[error("enabled hook config requires an executable")]
    ExecutableRequired,
    #[error("hook executable must use an absolute path")]
    ExecutableAbsolute,
    #[error("hook executable must be a regular file and not a symlink")]
    ExecutableRegular,
    #[error("hook supports at most 32 arguments of at most 4096 bytes each")]
    Arguments,
    #[error("hook events must be non-empty and unique")]
    Events,
    #[error("hook process stdin is unavailable")]
    Stdin,
    #[error("hook process exited unsuccessfully with code {0:?}")]
    Exit(Option<i32>),
    #[error("cannot start hook dispatcher: {0}")]
    Thread(std::io::Error),
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::time::Duration;

    use super::*;

    #[test]
    fn enabled_hook_requires_an_absolute_regular_executable() {
        let mut config = HookConfig {
            enabled: true,
            executable: Some(PathBuf::from("relative-command")),
            ..HookConfig::default()
        };
        assert!(matches!(
            validate(&config),
            Err(NotificationError::ExecutableAbsolute)
        ));
        config.events.push(MilestoneKind::BuildCompleted);
        config.enabled = false;
        assert!(matches!(validate(&config), Err(NotificationError::Events)));
    }

    #[test]
    fn dispatcher_passes_json_on_stdin_without_shell_expansion() {
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("capture.sh");
        let output = directory.path().join("event.json");
        fs::write(&executable, "#!/bin/sh\n/bin/cat > \"$1\"\n").unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        let dispatcher = HookDispatcher::from_config(HookConfig {
            schema_version: NOTIFICATION_SCHEMA_VERSION.to_owned(),
            enabled: true,
            executable: Some(executable),
            args: vec![output.display().to_string()],
            events: vec![MilestoneKind::BuildCompleted],
        })
        .unwrap()
        .unwrap();
        dispatcher.emit(MilestoneEvent::new(
            MilestoneKind::BuildCompleted,
            "demo",
            "BLD-1; touch should-not-run",
            "build ready",
            "100Z",
        ));

        for _ in 0..100 {
            if fs::metadata(&output).is_ok_and(|metadata| metadata.len() > 0) {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let event: MilestoneEvent = serde_json::from_slice(&fs::read(output).unwrap()).unwrap();
        assert_eq!(event.subject_id, "BLD-1; touch should-not-run");
        assert!(!directory.path().join("should-not-run").exists());
    }
}
