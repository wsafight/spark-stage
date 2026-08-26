use clap::ValueEnum;

use super::*;
use crate::domain::ReferenceSubjectKind;

#[derive(Debug, Clone, Copy, ValueEnum)]
enum SubjectKindArg {
    Character,
    Location,
}

impl From<SubjectKindArg> for ReferenceSubjectKind {
    fn from(value: SubjectKindArg) -> Self {
        match value {
            SubjectKindArg::Character => Self::Character,
            SubjectKindArg::Location => Self::Location,
        }
    }
}

#[derive(Debug, Args)]
pub(super) struct ReferencesArgs {
    #[command(subcommand)]
    command: ReferencesCommand,
}

#[derive(Debug, Subcommand)]
enum ReferencesCommand {
    /// List active and superseded references from project state.
    List(Target),
    /// Show the shots, takes, and builds affected by changing one subject's references.
    Impact {
        #[command(flatten)]
        target: Target,
        #[arg(long, value_enum)]
        kind: SubjectKindArg,
        #[arg(long, value_name = "SUBJECT_ID")]
        id: String,
    },
    /// Import an immutable reference for a character or location.
    Import {
        #[command(flatten)]
        target: Target,
        #[arg(long, value_enum)]
        kind: SubjectKindArg,
        #[arg(long, value_name = "SUBJECT_ID")]
        id: String,
        #[arg(long, value_name = "PATH")]
        file: PathBuf,
        /// Accept invalidation of the affected takes and builds.
        #[arg(long)]
        accept_impact: bool,
    },
    /// Replace one active reference while retaining its immutable predecessor.
    Replace {
        #[command(flatten)]
        target: Target,
        #[arg(long, value_name = "REFERENCE_ID")]
        reference: String,
        #[arg(long, value_name = "PATH")]
        file: PathBuf,
        /// Accept invalidation of the affected takes and builds.
        #[arg(long)]
        accept_impact: bool,
    },
    /// Verify all reference sizes and SHA-256 hashes.
    Verify(Target),
}

#[derive(Debug, Args)]
struct Target {
    #[arg(long, value_name = "PROJECT_ID")]
    project: String,
    #[command(flatten)]
    connection: ConnectionArgs,
    #[arg(long)]
    json: bool,
}

pub(super) fn execute_references(args: ReferencesArgs) -> Result<ExitCode, CliError> {
    let (target, command) = match args.command {
        ReferencesCommand::List(target) => (target, IpcCommand::ListReferences),
        ReferencesCommand::Impact { target, kind, id } => (
            target,
            IpcCommand::ReferenceImpact {
                subject_kind: kind.into(),
                subject_id: id,
            },
        ),
        ReferencesCommand::Import {
            target,
            kind,
            id,
            file,
            accept_impact,
        } => (
            target,
            IpcCommand::ImportReference {
                subject_kind: kind.into(),
                subject_id: id,
                source: absolute_input_path(&file)?,
                accept_impact,
            },
        ),
        ReferencesCommand::Replace {
            target,
            reference,
            file,
            accept_impact,
        } => (
            target,
            IpcCommand::ReplaceReference {
                reference_id: reference,
                source: absolute_input_path(&file)?,
                accept_impact,
            },
        ),
        ReferencesCommand::Verify(target) => (target, IpcCommand::VerifyReferences),
    };
    let client = WorkerClient::new(
        resolved_paths(&target.connection).socket,
        Some(target.project),
    );
    let revision = command
        .is_mutating()
        .then(|| current_revision(&client))
        .transpose()?;
    let reply = client.send(command, revision)?;
    print_reply(&reply, target.json)?;
    Ok(reply_exit_code(&reply))
}

fn absolute_input_path(path: &Path) -> Result<PathBuf, CliError> {
    if path.is_absolute() {
        Ok(path.to_owned())
    } else {
        std::env::current_dir()
            .map(|directory| directory.join(path))
            .map_err(CliError::Runtime)
    }
}
