use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use fs4::fs_std::FileExt;

use super::{StoreError, io_error};

pub struct ExclusiveFileLock {
    file: File,
    path: PathBuf,
}

impl ExclusiveFileLock {
    pub fn acquire(path: &Path) -> Result<Self, StoreError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|source| io_error(parent, source))?;
        }
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path)
            .map_err(|source| io_error(path, source))?;
        if !file
            .try_lock_exclusive()
            .map_err(|source| io_error(path, source))?
        {
            return Err(StoreError::LockBusy {
                path: path.to_owned(),
            });
        }
        file.set_len(0).map_err(|source| io_error(path, source))?;
        writeln!(file, "{}", std::process::id()).map_err(|source| io_error(path, source))?;
        file.sync_data().map_err(|source| io_error(path, source))?;
        Ok(Self {
            file,
            path: path.to_owned(),
        })
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for ExclusiveFileLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_exclusive_lock_is_rejected() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("worker.lock");
        let first = ExclusiveFileLock::acquire(&path).unwrap();
        let second = ExclusiveFileLock::acquire(&path);

        assert!(matches!(second, Err(StoreError::LockBusy { .. })));
        drop(first);
        assert!(ExclusiveFileLock::acquire(&path).is_ok());
    }
}
