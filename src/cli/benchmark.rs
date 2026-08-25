use std::process::ExitCode;

use clap::{Args, Subcommand};

use super::*;
use crate::benchmark::{BenchmarkSampleInput, initialize_h3_run, record_h3_sample, show_h3_run};

#[derive(Debug, Args)]
pub(super) struct BenchmarkArgs {
    #[command(subcommand)]
    command: BenchmarkFamily,
}

#[derive(Debug, Subcommand)]
enum BenchmarkFamily {
    /// Manage MiniMax H3 benchmark records without bypassing the production adapter.
    H3(H3Args),
}

#[derive(Debug, Args)]
struct H3Args {
    #[command(subcommand)]
    command: H3Command,
}

#[derive(Debug, Subcommand)]
enum H3Command {
    /// Freeze adapter, workflow, profile, and environment fingerprints.
    Init {
        #[arg(long, value_name = "PATH")]
        adapter_config: PathBuf,
        #[arg(long, value_name = "PATH")]
        environment_file: Option<PathBuf>,
        #[arg(long, value_name = "PATH")]
        data_dir: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// Append one DGX-produced sample without changing run verification status.
    Record {
        #[arg(long, value_name = "RUN_ID")]
        run: String,
        #[arg(long, value_name = "PATH")]
        sample: PathBuf,
        #[arg(long, value_name = "PATH")]
        data_dir: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// Show the immutable run metadata and all appended samples.
    Show {
        #[arg(long, value_name = "RUN_ID")]
        run: String,
        #[arg(long, value_name = "PATH")]
        data_dir: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
}

pub(super) fn execute_benchmark(args: BenchmarkArgs) -> Result<ExitCode, CliError> {
    let BenchmarkFamily::H3(args) = args.command;
    match args.command {
        H3Command::Init {
            adapter_config,
            environment_file,
            data_dir,
            json,
        } => {
            let paths = AppPaths::resolve(data_dir, None);
            let run = initialize_h3_run(
                &paths.benchmarks_dir,
                &adapter_config,
                environment_file.as_deref(),
            )?;
            if json {
                println!("{}", serde_json::to_string_pretty(&run)?);
            } else {
                println!("{}: status=prepared", run.run_id);
            }
        }
        H3Command::Record {
            run,
            sample,
            data_dir,
            json,
        } => {
            let paths = AppPaths::resolve(data_dir, None);
            let source = read_text(&sample)?;
            let input: BenchmarkSampleInput = serde_json::from_str(&source).map_err(|error| {
                CliError::InvalidInput(format!(
                    "cannot decode benchmark sample `{}`: {error}",
                    sample.display()
                ))
            })?;
            let recorded = record_h3_sample(&paths.benchmarks_dir, &run, input)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&recorded)?);
            } else {
                println!("{}: appended to {}", recorded.sample_id, recorded.run_id);
            }
        }
        H3Command::Show {
            run,
            data_dir,
            json,
        } => {
            let paths = AppPaths::resolve(data_dir, None);
            let report = show_h3_run(&paths.benchmarks_dir, &run)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!(
                    "{}: status=prepared, adapter={}, samples={}",
                    report.run.run_id,
                    report.run.adapter,
                    report.samples.len()
                );
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}
