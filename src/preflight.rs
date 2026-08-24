use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::adapter::{
    CameraAdapter, CapabilityReport, CapabilityStatus, ComfyAdapter, ComfyAdapterConfig,
};

pub const PREFLIGHT_SCHEMA_VERSION: &str = "1.0";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    Pass,
    Warn,
    Fail,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreflightCheck {
    pub name: String,
    pub status: CheckStatus,
    pub code: String,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdapterPreflight {
    pub config: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report: Option<CapabilityReport>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreflightReport {
    pub schema_version: String,
    pub ready: bool,
    pub checks: Vec<PreflightCheck>,
    pub adapters: Vec<AdapterPreflight>,
}

pub async fn run(
    data_home: &Path,
    adapter_configs: &[PathBuf],
    minimum_free_bytes: u64,
) -> PreflightReport {
    let mut checks = vec![
        command_check(
            "nvidia",
            "nvidia-smi",
            &[
                "--query-gpu=name,driver_version,memory.total",
                "--format=csv,noheader",
            ],
            "NVIDIA_RUNTIME_READY",
            "NVIDIA_RUNTIME_UNAVAILABLE",
        ),
        command_check(
            "ffmpeg",
            "ffmpeg",
            &["-version"],
            "FFMPEG_READY",
            "FFMPEG_UNAVAILABLE",
        ),
        command_check(
            "ffprobe",
            "ffprobe",
            &["-version"],
            "FFPROBE_READY",
            "FFPROBE_UNAVAILABLE",
        ),
        disk_check(data_home, minimum_free_bytes),
    ];
    checks.push(ffmpeg_capability_check());

    let mut adapters = Vec::new();
    if adapter_configs.is_empty() {
        checks.push(PreflightCheck {
            name: "camera_adapter".to_owned(),
            status: CheckStatus::Fail,
            code: "ADAPTER_CONFIG_MISSING".to_owned(),
            detail: "no adapter config found; create adapters/minimax-h3-comfy.yaml from the disabled example".to_owned(),
        });
    }

    for path in adapter_configs {
        match ComfyAdapterConfig::load(path).and_then(ComfyAdapter::new) {
            Ok(adapter) => {
                let capability = adapter.preflight().await;
                let t2v_verified = capability
                    .operations
                    .get("t2v")
                    .is_some_and(|operation| operation.status == CapabilityStatus::Verified);
                let (status, code, detail) = if !adapter.config().enabled {
                    (
                        CheckStatus::Fail,
                        "ADAPTER_DISABLED",
                        "adapter config is intentionally disabled".to_owned(),
                    )
                } else if !capability.available {
                    (
                        CheckStatus::Fail,
                        "ADAPTER_UNAVAILABLE",
                        "ComfyUI or its workflow is unavailable".to_owned(),
                    )
                } else if !capability.binding_errors.is_empty()
                    || !capability.missing_nodes.is_empty()
                {
                    (
                        CheckStatus::Fail,
                        "WORKFLOW_INVALID",
                        "workflow bindings or installed ComfyUI nodes do not match".to_owned(),
                    )
                } else if !t2v_verified {
                    (
                        CheckStatus::Fail,
                        "T2V_UNVERIFIED",
                        "workflow is reachable but T2V has no recorded smoke test".to_owned(),
                    )
                } else {
                    (
                        CheckStatus::Pass,
                        "ADAPTER_READY",
                        "adapter is reachable and T2V is recorded as smoke-tested".to_owned(),
                    )
                };
                checks.push(PreflightCheck {
                    name: format!("adapter:{}", capability.adapter),
                    status,
                    code: code.to_owned(),
                    detail,
                });
                adapters.push(AdapterPreflight {
                    config: path.clone(),
                    report: Some(capability),
                    error: None,
                });
            }
            Err(error) => {
                checks.push(PreflightCheck {
                    name: format!("adapter:{}", path.display()),
                    status: CheckStatus::Fail,
                    code: "ADAPTER_CONFIG_INVALID".to_owned(),
                    detail: error.to_string(),
                });
                adapters.push(AdapterPreflight {
                    config: path.clone(),
                    report: None,
                    error: Some(error.to_string()),
                });
            }
        }
    }

    PreflightReport {
        schema_version: PREFLIGHT_SCHEMA_VERSION.to_owned(),
        ready: checks.iter().all(|check| check.status != CheckStatus::Fail),
        checks,
        adapters,
    }
}

fn ffmpeg_capability_check() -> PreflightCheck {
    match crate::media::missing_runtime_capabilities() {
        Ok(missing) if missing.is_empty() => PreflightCheck {
            name: "ffmpeg_features".to_owned(),
            status: CheckStatus::Pass,
            code: "FFMPEG_CAPABILITIES_READY".to_owned(),
            detail: "required media filters and muxers are available".to_owned(),
        },
        Ok(missing) => PreflightCheck {
            name: "ffmpeg_features".to_owned(),
            status: CheckStatus::Fail,
            code: "FFMPEG_CAPABILITY_MISSING".to_owned(),
            detail: missing.join(", "),
        },
        Err(error) => PreflightCheck {
            name: "ffmpeg_features".to_owned(),
            status: CheckStatus::Fail,
            code: "FFMPEG_CAPABILITY_PROBE_FAILED".to_owned(),
            detail: error.to_string(),
        },
    }
}

fn command_check(
    name: &str,
    program: &str,
    args: &[&str],
    pass_code: &str,
    failure_code: &str,
) -> PreflightCheck {
    match Command::new(program).args(args).output() {
        Ok(output) if output.status.success() => {
            let first_line = String::from_utf8_lossy(&output.stdout)
                .lines()
                .next()
                .unwrap_or("available")
                .trim()
                .to_owned();
            PreflightCheck {
                name: name.to_owned(),
                status: CheckStatus::Pass,
                code: pass_code.to_owned(),
                detail: first_line,
            }
        }
        Ok(output) => PreflightCheck {
            name: name.to_owned(),
            status: CheckStatus::Fail,
            code: failure_code.to_owned(),
            detail: String::from_utf8_lossy(&output.stderr)
                .lines()
                .next()
                .unwrap_or("command returned a non-zero status")
                .trim()
                .to_owned(),
        },
        Err(error) => PreflightCheck {
            name: name.to_owned(),
            status: CheckStatus::Fail,
            code: failure_code.to_owned(),
            detail: error.to_string(),
        },
    }
}

fn disk_check(data_home: &Path, minimum_free_bytes: u64) -> PreflightCheck {
    let Some(probe_path) = nearest_existing_path(data_home) else {
        return PreflightCheck {
            name: "disk".to_owned(),
            status: CheckStatus::Fail,
            code: "DISK_PATH_UNAVAILABLE".to_owned(),
            detail: format!("no existing parent for {}", data_home.display()),
        };
    };
    match fs4::available_space(&probe_path) {
        Ok(available) => {
            let available_gib = available as f64 / 1024_f64.powi(3);
            let required_gib = minimum_free_bytes as f64 / 1024_f64.powi(3);
            let enough = available >= minimum_free_bytes;
            PreflightCheck {
                name: "disk".to_owned(),
                status: if enough {
                    CheckStatus::Pass
                } else {
                    CheckStatus::Fail
                },
                code: if enough {
                    "DISK_SPACE_READY"
                } else {
                    "DISK_SPACE_LOW"
                }
                .to_owned(),
                detail: format!(
                    "{available_gib:.1} GiB available at {}; requires {required_gib:.1} GiB",
                    probe_path.display()
                ),
            }
        }
        Err(error) => PreflightCheck {
            name: "disk".to_owned(),
            status: CheckStatus::Fail,
            code: "DISK_PROBE_FAILED".to_owned(),
            detail: error.to_string(),
        },
    }
}

fn nearest_existing_path(path: &Path) -> Option<PathBuf> {
    path.ancestors()
        .find(|candidate| candidate.exists())
        .map(Path::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disk_probe_uses_existing_parent_without_creating_data_home() {
        let directory = tempfile::tempdir().unwrap();
        let missing = directory.path().join("not-created/data");
        let check = disk_check(&missing, 0);

        assert_eq!(check.status, CheckStatus::Pass);
        assert!(!missing.exists());
    }

    #[tokio::test]
    async fn no_adapter_is_not_ready() {
        let directory = tempfile::tempdir().unwrap();
        let report = run(directory.path(), &[], 0).await;

        assert!(!report.ready);
        assert!(
            report
                .checks
                .iter()
                .any(|check| check.code == "ADAPTER_CONFIG_MISSING")
        );
    }
}
