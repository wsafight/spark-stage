use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppPaths {
    pub data_home: PathBuf,
    pub runtime_dir: PathBuf,
    pub projects_dir: PathBuf,
    pub benchmarks_dir: PathBuf,
    pub socket: PathBuf,
}

impl AppPaths {
    #[must_use]
    pub fn resolve(data_home: Option<PathBuf>, socket: Option<PathBuf>) -> Self {
        let data_home = data_home.unwrap_or_else(default_data_home);
        let runtime_dir = data_home.join("runtime");
        Self {
            projects_dir: data_home.join("projects"),
            benchmarks_dir: data_home.join("benchmarks/h3"),
            socket: socket.unwrap_or_else(|| runtime_dir.join("worker.sock")),
            data_home,
            runtime_dir,
        }
    }

    #[must_use]
    pub fn project_dir(&self, project_id: &str) -> PathBuf {
        self.projects_dir.join(project_id)
    }

    #[must_use]
    pub fn worker_lock(&self) -> PathBuf {
        self.runtime_dir.join("worker.lock")
    }

    #[must_use]
    pub fn queue_file(&self) -> PathBuf {
        self.runtime_dir.join("queue.json")
    }

    #[must_use]
    pub fn command_journal(&self) -> PathBuf {
        self.runtime_dir.join("commands.jsonl")
    }
}

#[must_use]
pub fn default_data_home() -> PathBuf {
    if let Some(path) = std::env::var_os("SPARKSTAGE_DATA_HOME") {
        return PathBuf::from(path);
    }
    if let Some(path) = std::env::var_os("XDG_DATA_HOME") {
        return PathBuf::from(path).join("sparkstage");
    }
    if let Some(user_home) = std::env::var_os("HOME") {
        return PathBuf::from(user_home).join(".local/share/sparkstage");
    }
    PathBuf::from("/tmp/sparkstage")
}

#[must_use]
pub fn path_is_within(path: &Path, parent: &Path) -> bool {
    path.starts_with(parent)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_data_home_controls_all_runtime_paths() {
        let paths = AppPaths::resolve(Some(PathBuf::from("/data/sparkstage")), None);
        assert_eq!(
            paths.socket,
            PathBuf::from("/data/sparkstage/runtime/worker.sock")
        );
        assert_eq!(
            paths.project_dir("rain"),
            PathBuf::from("/data/sparkstage/projects/rain")
        );
    }
}
