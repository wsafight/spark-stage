mod app;
pub(crate) mod backend;
pub(crate) mod protocol;
mod terminal;
mod ui;

use std::path::PathBuf;
use std::time::Duration;

pub use backend::BackendError;
pub use protocol::*;
pub use terminal::TuiError;

#[derive(Debug, Clone)]
pub struct TuiOptions {
    pub socket: PathBuf,
    pub project_id: Option<String>,
    pub refresh_interval: Duration,
}

pub fn run(options: TuiOptions) -> Result<(), terminal::TuiError> {
    terminal::run(options)
}

#[must_use]
pub fn default_socket_path() -> PathBuf {
    if let Some(path) = std::env::var_os("SPARKSTAGE_SOCKET") {
        return PathBuf::from(path);
    }
    crate::paths::AppPaths::resolve(None, None).socket
}
