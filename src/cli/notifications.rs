use super::*;
use crate::notifications::{HookConfig, validate};

#[derive(Debug, Args)]
pub(super) struct NotificationsArgs {
    #[command(subcommand)]
    command: NotificationsCommand,
}

#[derive(Debug, Subcommand)]
enum NotificationsCommand {
    /// Print or write a disabled config containing every supported milestone.
    Default {
        #[arg(long, value_name = "PATH")]
        output: Option<PathBuf>,
    },
    /// Validate a hook config and its executable without installing it.
    Validate {
        #[arg(long, value_name = "PATH")]
        config: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Install a validated hook config for subsequent worker starts.
    Apply {
        #[arg(long, value_name = "PATH")]
        config: PathBuf,
        #[arg(long, value_name = "PATH")]
        data_dir: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// Show the installed hook config.
    Show {
        #[arg(long, value_name = "PATH")]
        data_dir: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// Disable installed hooks while retaining the command and event selection.
    Disable {
        #[arg(long, value_name = "PATH")]
        data_dir: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Serialize)]
struct ConfigReport<'a> {
    valid: bool,
    installed_path: Option<&'a Path>,
    config: &'a HookConfig,
}

pub(super) fn execute_notifications(args: NotificationsArgs) -> Result<ExitCode, CliError> {
    match args.command {
        NotificationsCommand::Default { output } => {
            let encoded = encode(&HookConfig::default())?;
            if let Some(path) = output {
                write_atomic(&path, encoded.as_bytes())?;
            } else {
                print!("{encoded}");
            }
        }
        NotificationsCommand::Validate { config, json } => {
            let config = HookConfig::load(&config)?;
            print_config(&config, None, json)?;
        }
        NotificationsCommand::Apply {
            config,
            data_dir,
            json,
        } => {
            let config = HookConfig::load(&config)?;
            let destination = AppPaths::resolve(data_dir, None).notifications_file();
            write_atomic(&destination, encode(&config)?.as_bytes())?;
            print_config(&config, Some(&destination), json)?;
        }
        NotificationsCommand::Show { data_dir, json } => {
            let path = AppPaths::resolve(data_dir, None).notifications_file();
            let config = HookConfig::load(&path)?;
            print_config(&config, Some(&path), json)?;
        }
        NotificationsCommand::Disable { data_dir, json } => {
            let path = AppPaths::resolve(data_dir, None).notifications_file();
            let mut config = if path.is_file() {
                HookConfig::load(&path)?
            } else {
                HookConfig::default()
            };
            config.enabled = false;
            validate(&config)?;
            write_atomic(&path, encode(&config)?.as_bytes())?;
            print_config(&config, Some(&path), json)?;
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn encode(config: &HookConfig) -> Result<String, CliError> {
    let mut encoded = serde_json::to_string_pretty(config)?;
    encoded.push('\n');
    Ok(encoded)
}

fn print_config(
    config: &HookConfig,
    installed_path: Option<&Path>,
    machine_readable: bool,
) -> Result<(), CliError> {
    if machine_readable {
        println!(
            "{}",
            serde_json::to_string_pretty(&ConfigReport {
                valid: true,
                installed_path,
                config,
            })?
        );
    } else {
        println!(
            "notifications: enabled={}, executable={}, events={}",
            config.enabled,
            config
                .executable
                .as_ref()
                .map_or_else(|| "none".to_owned(), |path| path.display().to_string()),
            config.events.len()
        );
        if let Some(path) = installed_path {
            println!("config: {}", path.display());
        }
    }
    Ok(())
}
