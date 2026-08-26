use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::validation::{SUPPORTED_SCHEMA_VERSION, validate_json};

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvaluationSuite {
    pub schema_version: String,
    pub cases: Vec<EvaluationExpectation>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvaluationExpectation {
    pub bundle: PathBuf,
    pub valid: bool,
    pub project_id: Option<String>,
    pub shots: Option<usize>,
    pub duration_seconds: Option<u32>,
    pub agent_host: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub issue_codes: Vec<String>,
    #[serde(default = "default_quality_sample")]
    pub quality_sample: bool,
    #[serde(default)]
    pub repair_count: u32,
}

const fn default_quality_sample() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvaluationReport {
    pub schema_version: String,
    pub suite_schema_version: String,
    pub passed: bool,
    pub total_cases: usize,
    pub expectation_matches: usize,
    pub valid_bundles: usize,
    pub invalid_bundles: usize,
    pub quality_samples: usize,
    pub first_pass_samples: usize,
    pub first_pass_valid: usize,
    pub first_pass_valid_rate: f64,
    pub total_repairs: u32,
    pub issue_code_counts: BTreeMap<String, usize>,
    pub agents: Vec<AgentEvaluation>,
    pub cases: Vec<EvaluationCaseResult>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentEvaluation {
    pub agent_host: String,
    pub model: String,
    pub samples: usize,
    pub valid: usize,
    pub first_pass_samples: usize,
    pub first_pass_valid: usize,
    pub repairs: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvaluationCaseResult {
    pub bundle: PathBuf,
    pub expectation_matched: bool,
    pub valid: bool,
    pub project_id: Option<String>,
    pub shots: Option<usize>,
    pub duration_seconds: Option<u32>,
    pub agent_host: Option<String>,
    pub model: Option<String>,
    pub quality_sample: bool,
    pub repair_count: u32,
    pub issue_codes: Vec<String>,
}

pub fn evaluate_suite(root: &Path, suite_path: &Path) -> Result<EvaluationReport, EvaluationError> {
    let source = fs::read(suite_path).map_err(|source| EvaluationError::Io {
        path: suite_path.to_owned(),
        source,
    })?;
    let suite: EvaluationSuite =
        serde_json::from_slice(&source).map_err(|source| EvaluationError::Decode {
            path: suite_path.to_owned(),
            source,
        })?;
    if suite.schema_version != SUPPORTED_SCHEMA_VERSION {
        return Err(EvaluationError::Schema(suite.schema_version));
    }
    if suite.cases.is_empty() {
        return Err(EvaluationError::Empty);
    }

    let mut cases = Vec::with_capacity(suite.cases.len());
    let mut issue_code_counts = BTreeMap::new();
    let mut agents: BTreeMap<(String, String), AgentEvaluation> = BTreeMap::new();
    for expected in suite.cases {
        let path = resolve_bundle_path(root, suite_path, &expected.bundle);
        let bundle_source = fs::read_to_string(&path).map_err(|source| EvaluationError::Io {
            path: path.clone(),
            source,
        })?;
        let validation = validate_json(&bundle_source);
        let actual_valid = validation.is_valid();
        let issue_codes = validation
            .issues
            .iter()
            .map(|issue| issue.code.to_owned())
            .collect::<Vec<_>>();
        for code in &issue_codes {
            *issue_code_counts.entry(code.clone()).or_insert(0) += 1;
        }
        let bundle = validation.bundle.as_ref();
        let project_id = bundle.map(|bundle| bundle.project.id.clone());
        let shots = bundle.map(|bundle| bundle.shots.len());
        let duration_seconds =
            bundle.map(|bundle| bundle.shots.iter().map(|shot| shot.duration).sum::<u32>());
        let actual_agent = bundle
            .and_then(|bundle| bundle.authoring.as_ref())
            .and_then(|authoring| authoring.agent_host.clone())
            .or_else(|| expected.agent_host.clone());
        let actual_model = bundle
            .and_then(|bundle| bundle.authoring.as_ref())
            .and_then(|authoring| authoring.model.clone())
            .or_else(|| expected.model.clone());
        let expectation_matched = actual_valid == expected.valid
            && project_id == expected.project_id
            && shots == expected.shots
            && duration_seconds == expected.duration_seconds
            && actual_agent == expected.agent_host
            && expected
                .model
                .as_ref()
                .is_none_or(|model| actual_model.as_ref() == Some(model))
            && issue_codes == expected.issue_codes;
        if expected.quality_sample {
            let key = (
                actual_agent
                    .clone()
                    .unwrap_or_else(|| "unknown-agent".to_owned()),
                actual_model
                    .clone()
                    .unwrap_or_else(|| "unknown-model".to_owned()),
            );
            let agent = agents.entry(key.clone()).or_insert(AgentEvaluation {
                agent_host: key.0,
                model: key.1,
                samples: 0,
                valid: 0,
                first_pass_samples: 0,
                first_pass_valid: 0,
                repairs: 0,
            });
            agent.samples += 1;
            agent.valid += usize::from(actual_valid);
            agent.repairs = agent.repairs.saturating_add(expected.repair_count);
            if expected.repair_count == 0 {
                agent.first_pass_samples += 1;
                agent.first_pass_valid += usize::from(actual_valid);
            }
        }
        cases.push(EvaluationCaseResult {
            bundle: expected.bundle,
            expectation_matched,
            valid: actual_valid,
            project_id,
            shots,
            duration_seconds,
            agent_host: actual_agent,
            model: actual_model,
            quality_sample: expected.quality_sample,
            repair_count: expected.repair_count,
            issue_codes,
        });
    }

    let total_cases = cases.len();
    let expectation_matches = cases.iter().filter(|case| case.expectation_matched).count();
    let valid_bundles = cases.iter().filter(|case| case.valid).count();
    let quality = cases
        .iter()
        .filter(|case| case.quality_sample)
        .collect::<Vec<_>>();
    let first_pass = quality
        .iter()
        .filter(|case| case.repair_count == 0)
        .collect::<Vec<_>>();
    let first_pass_valid = first_pass.iter().filter(|case| case.valid).count();
    let first_pass_valid_rate = if first_pass.is_empty() {
        0.0
    } else {
        first_pass_valid as f64 / first_pass.len() as f64
    };
    Ok(EvaluationReport {
        schema_version: SUPPORTED_SCHEMA_VERSION.to_owned(),
        suite_schema_version: suite.schema_version,
        passed: expectation_matches == total_cases,
        total_cases,
        expectation_matches,
        valid_bundles,
        invalid_bundles: total_cases - valid_bundles,
        quality_samples: quality.len(),
        first_pass_samples: first_pass.len(),
        first_pass_valid,
        first_pass_valid_rate,
        total_repairs: quality.iter().map(|case| case.repair_count).sum(),
        issue_code_counts,
        agents: agents.into_values().collect(),
        cases,
    })
}

fn resolve_bundle_path(root: &Path, suite_path: &Path, bundle: &Path) -> PathBuf {
    if bundle.is_absolute() {
        return bundle.to_owned();
    }
    let rooted = root.join(bundle);
    if rooted.is_file() {
        rooted
    } else {
        suite_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(bundle)
    }
}

#[derive(Debug, Error)]
pub enum EvaluationError {
    #[error("cannot access `{path}`: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("cannot decode evaluation suite `{path}`: {source}")]
    Decode {
        path: PathBuf,
        source: serde_json::Error,
    },
    #[error("unsupported evaluation suite schema `{0}`")]
    Schema(String),
    #[error("evaluation suite must contain at least one case")]
    Empty,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checked_in_suite_reports_expectations_and_agent_metrics() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let report = evaluate_suite(
            root,
            &root.join("tests/fixtures/agent-script-bundles/expectations.json"),
        )
        .unwrap();

        assert!(report.passed);
        assert_eq!(report.total_cases, 3);
        assert_eq!(report.expectation_matches, 3);
        assert_eq!(report.quality_samples, 2);
        assert_eq!(report.first_pass_valid_rate, 1.0);
        assert_eq!(report.agents.len(), 2);
        assert_eq!(report.issue_code_counts["JSON_CONTRACT"], 1);
    }
}
