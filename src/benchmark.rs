use std::collections::BTreeMap;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use ulid::Ulid;

use crate::adapter::{AdapterError, ComfyAdapter, ComfyAdapterConfig};
use crate::store::{
    StoreError, append_jsonl, read_json, read_jsonl, sha256_file, sha256_json, write_json_atomic,
    write_text_atomic,
};

pub const BENCHMARK_SCHEMA_VERSION: &str = "1.0";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BenchmarkStatus {
    Prepared,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BenchmarkRun {
    pub schema_version: String,
    pub run_id: String,
    pub status: BenchmarkStatus,
    pub adapter: String,
    pub adapter_fingerprint: String,
    pub workflow_fingerprint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_fingerprint: Option<String>,
    pub profile_fingerprints: BTreeMap<String, String>,
    pub environment_fingerprint: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BenchmarkSampleInput {
    pub profile: String,
    pub operation: String,
    pub seed: u64,
    pub width: u32,
    pub height: u32,
    pub frames: u32,
    pub fps: u32,
    pub steps: u32,
    pub elapsed_milliseconds: u64,
    #[serde(default)]
    pub cold_start: bool,
    pub job_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peak_memory_bytes: Option<u64>,
    #[serde(default)]
    pub stage_milliseconds: BTreeMap<String, u64>,
    #[serde(default)]
    pub quality_metrics: BTreeMap<String, f64>,
    pub evidence: Vec<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BenchmarkSample {
    pub schema_version: String,
    pub sample_id: String,
    pub run_id: String,
    pub recorded_at: String,
    #[serde(flatten)]
    pub input: BenchmarkSampleInput,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BenchmarkReport {
    pub run: BenchmarkRun,
    pub samples: Vec<BenchmarkSample>,
}

#[derive(Debug, Error)]
pub enum BenchmarkError {
    #[error("benchmark run `{0}` does not exist")]
    RunNotFound(String),
    #[error("invalid benchmark run id `{0}`")]
    InvalidRunId(String),
    #[error("invalid benchmark sample: {0}")]
    InvalidSample(String),
    #[error("cannot access `{path}`: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("cannot decode environment file `{path}`: {source}")]
    EnvironmentDecode {
        path: PathBuf,
        source: serde_json::Error,
    },
    #[error(transparent)]
    Adapter(#[from] AdapterError),
    #[error(transparent)]
    Store(#[from] StoreError),
}

pub fn initialize_h3_run(
    benchmarks_dir: &Path,
    adapter_config_path: &Path,
    environment_file: Option<&Path>,
) -> Result<BenchmarkRun, BenchmarkError> {
    let config = ComfyAdapterConfig::load(adapter_config_path)?;
    let adapter = ComfyAdapter::new(config.clone())?;
    let workflow_fingerprint = adapter.validate_local_workflow()?;
    let workflow: Value = read_json(&config.workflow)?;
    let environment = load_environment(environment_file)?;
    let run_id = format!("H3RUN-{}", Ulid::new());
    let run = BenchmarkRun {
        schema_version: BENCHMARK_SCHEMA_VERSION.to_owned(),
        run_id: run_id.clone(),
        status: BenchmarkStatus::Prepared,
        adapter: config.adapter.clone(),
        adapter_fingerprint: sha256_file(adapter_config_path)?,
        workflow_fingerprint,
        model_fingerprint: config.model_fingerprint.clone(),
        profile_fingerprints: config
            .profiles
            .iter()
            .map(|(name, profile)| Ok((name.clone(), sha256_json(profile)?)))
            .collect::<Result<_, StoreError>>()?,
        environment_fingerprint: sha256_json(&environment)?,
        created_at: timestamp(),
    };

    fs::create_dir_all(benchmarks_dir).map_err(|source| io_error(benchmarks_dir, source))?;
    let target = benchmarks_dir.join(&run_id);
    let staging = benchmarks_dir.join(format!(".{run_id}.new-{}", Ulid::new()));
    let result = (|| {
        fs::create_dir(&staging).map_err(|source| io_error(&staging, source))?;
        fs::create_dir(staging.join("profiler"))
            .map_err(|source| io_error(staging.join("profiler"), source))?;
        fs::create_dir(staging.join("output"))
            .map_err(|source| io_error(staging.join("output"), source))?;
        write_json_atomic(&staging.join("environment.json"), &environment)?;
        write_json_atomic(&staging.join("workflow-api.json"), &workflow)?;
        fs::copy(adapter_config_path, staging.join("adapter-config.yaml"))
            .map_err(|source| io_error(staging.join("adapter-config.yaml"), source))?;
        write_json_atomic(&staging.join("run.json"), &run)?;
        write_text_atomic(&staging.join("samples.jsonl"), "")?;
        fs::rename(&staging, &target).map_err(|source| io_error(&target, source))?;
        File::open(benchmarks_dir)
            .and_then(|directory| directory.sync_all())
            .map_err(|source| io_error(benchmarks_dir, source))?;
        Ok::<(), BenchmarkError>(())
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&staging);
    }
    result?;
    Ok(run)
}

pub fn record_h3_sample(
    benchmarks_dir: &Path,
    run_id: &str,
    input: BenchmarkSampleInput,
) -> Result<BenchmarkSample, BenchmarkError> {
    let run_dir = run_directory(benchmarks_dir, run_id)?;
    let run = read_run(&run_dir, run_id)?;
    validate_sample(&run, &input)?;
    let sample = BenchmarkSample {
        schema_version: BENCHMARK_SCHEMA_VERSION.to_owned(),
        sample_id: format!("H3SAMPLE-{}", Ulid::new()),
        run_id: run_id.to_owned(),
        recorded_at: timestamp(),
        input,
    };
    append_jsonl(&run_dir.join("samples.jsonl"), &sample)?;
    Ok(sample)
}

pub fn show_h3_run(benchmarks_dir: &Path, run_id: &str) -> Result<BenchmarkReport, BenchmarkError> {
    let run_dir = run_directory(benchmarks_dir, run_id)?;
    Ok(BenchmarkReport {
        run: read_run(&run_dir, run_id)?,
        samples: read_jsonl(&run_dir.join("samples.jsonl"))?,
    })
}

fn read_run(run_dir: &Path, run_id: &str) -> Result<BenchmarkRun, BenchmarkError> {
    let path = run_dir.join("run.json");
    let run: BenchmarkRun = match read_json(&path) {
        Ok(run) => run,
        Err(StoreError::Io { source, .. }) if source.kind() == std::io::ErrorKind::NotFound => {
            return Err(BenchmarkError::RunNotFound(run_id.to_owned()));
        }
        Err(error) => return Err(error.into()),
    };
    if run.schema_version != BENCHMARK_SCHEMA_VERSION || run.run_id != run_id {
        return Err(BenchmarkError::InvalidRunId(run_id.to_owned()));
    }
    Ok(run)
}

fn run_directory(benchmarks_dir: &Path, run_id: &str) -> Result<PathBuf, BenchmarkError> {
    let suffix = run_id.strip_prefix("H3RUN-").unwrap_or_default();
    if suffix.len() != 26 || !suffix.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
        return Err(BenchmarkError::InvalidRunId(run_id.to_owned()));
    }
    Ok(benchmarks_dir.join(run_id))
}

fn validate_sample(
    run: &BenchmarkRun,
    sample: &BenchmarkSampleInput,
) -> Result<(), BenchmarkError> {
    if !run.profile_fingerprints.contains_key(&sample.profile) {
        return Err(BenchmarkError::InvalidSample(format!(
            "profile `{}` was not frozen by the run",
            sample.profile
        )));
    }
    if !matches!(sample.operation.as_str(), "t2v" | "i2v" | "flf2v" | "r2v") {
        return Err(BenchmarkError::InvalidSample(format!(
            "unsupported operation `{}`",
            sample.operation
        )));
    }
    if sample.width == 0
        || sample.height == 0
        || sample.frames == 0
        || sample.fps == 0
        || sample.steps == 0
        || sample.elapsed_milliseconds == 0
    {
        return Err(BenchmarkError::InvalidSample(
            "dimensions, frames, fps, steps, and elapsed time must be positive".to_owned(),
        ));
    }
    if sample.job_id.trim().is_empty() {
        return Err(BenchmarkError::InvalidSample(
            "job_id must link the sample to a production adapter job".to_owned(),
        ));
    }
    if sample.evidence.is_empty()
        || sample
            .evidence
            .iter()
            .any(|path| path.as_os_str().is_empty())
    {
        return Err(BenchmarkError::InvalidSample(
            "at least one non-empty evidence path is required".to_owned(),
        ));
    }
    Ok(())
}

fn load_environment(path: Option<&Path>) -> Result<Value, BenchmarkError> {
    if let Some(path) = path {
        let source = fs::read_to_string(path).map_err(|source| io_error(path, source))?;
        return serde_json::from_str(&source).map_err(|source| BenchmarkError::EnvironmentDecode {
            path: path.to_owned(),
            source,
        });
    }
    Ok(serde_json::json!({
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "sparkstage_version": env!("CARGO_PKG_VERSION"),
        "capture": "sparkstage_local_control_plane"
    }))
}

fn timestamp() -> String {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}.{:03}Z", duration.as_secs(), duration.subsec_millis())
}

fn io_error(path: impl Into<PathBuf>, source: std::io::Error) -> BenchmarkError {
    BenchmarkError::Io {
        path: path.into(),
        source,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let directory = tempfile::tempdir().unwrap();
        let workflow = directory.path().join("workflow.json");
        write_json_atomic(
            &workflow,
            &serde_json::json!({
                "1": {"class_type": "Text", "inputs": {"text": ""}},
                "2": {"class_type": "Seed", "inputs": {"seed": 0}},
                "3": {"class_type": "Output", "inputs": {"filename_prefix": "out"}}
            }),
        )
        .unwrap();
        let adapter = directory.path().join("adapter.yaml");
        fs::write(
            &adapter,
            format!(
                "schema_version: '1.0'\nadapter: test\nenabled: false\nendpoint: http://127.0.0.1:8188\nworkflow: {}\noutput_node: '3'\nmodel_fingerprint: model\nbindings:\n  prompt: {{ node: '1', input: text }}\n  seed: {{ node: '2', input: seed }}\n  output_prefix: {{ node: '3', input: filename_prefix }}\nprofiles:\n  baseline: {{}}\nverified_operations: []\n",
                workflow.display()
            ),
        )
        .unwrap();
        (directory, adapter, workflow)
    }

    fn sample() -> BenchmarkSampleInput {
        BenchmarkSampleInput {
            profile: "baseline".to_owned(),
            operation: "t2v".to_owned(),
            seed: 7,
            width: 960,
            height: 544,
            frames: 121,
            fps: 24,
            steps: 12,
            elapsed_milliseconds: 240_000,
            cold_start: false,
            job_id: "JOB-01ARZ3NDEKTSV4RRFFQ69G5FAV".to_owned(),
            peak_memory_bytes: Some(32_000_000_000),
            stage_milliseconds: BTreeMap::new(),
            quality_metrics: BTreeMap::new(),
            evidence: vec![PathBuf::from("telemetry.csv")],
            notes: None,
        }
    }

    #[test]
    fn run_freezes_fingerprints_and_remains_prepared_after_record() {
        let (directory, adapter, _workflow) = fixture();
        let root = directory.path().join("benchmarks");
        let run = initialize_h3_run(&root, &adapter, None).unwrap();
        assert_eq!(run.status, BenchmarkStatus::Prepared);
        assert!(run.profile_fingerprints.contains_key("baseline"));
        let run_path = root.join(&run.run_id).join("run.json");
        let frozen = fs::read(&run_path).unwrap();

        let recorded = record_h3_sample(&root, &run.run_id, sample()).unwrap();
        assert_eq!(recorded.run_id, run.run_id);
        assert_eq!(fs::read(run_path).unwrap(), frozen);
        let report = show_h3_run(&root, &run.run_id).unwrap();
        assert_eq!(report.run.status, BenchmarkStatus::Prepared);
        assert_eq!(report.samples, [recorded]);
    }

    #[test]
    fn sample_requires_frozen_profile_job_and_evidence() {
        let (directory, adapter, _workflow) = fixture();
        let root = directory.path().join("benchmarks");
        let run = initialize_h3_run(&root, &adapter, None).unwrap();

        let mut invalid = sample();
        invalid.profile = "unknown".to_owned();
        assert!(matches!(
            record_h3_sample(&root, &run.run_id, invalid),
            Err(BenchmarkError::InvalidSample(_))
        ));
        let mut invalid = sample();
        invalid.evidence.clear();
        assert!(matches!(
            record_h3_sample(&root, &run.run_id, invalid),
            Err(BenchmarkError::InvalidSample(_))
        ));
    }

    #[test]
    fn run_id_cannot_escape_benchmark_root() {
        let directory = tempfile::tempdir().unwrap();
        assert!(matches!(
            show_h3_run(directory.path(), "../project"),
            Err(BenchmarkError::InvalidRunId(_))
        ));
    }
}
