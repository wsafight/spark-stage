use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;

use serde::Serialize;
use serde::de::DeserializeOwned;
use sha2::{Digest, Sha256};
use ulid::Ulid;

use super::{StoreError, io_error};

pub fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T, StoreError> {
    let encoded = fs::read(path).map_err(|source| io_error(path, source))?;
    serde_json::from_slice(&encoded).map_err(|source| StoreError::Decode {
        path: path.to_owned(),
        source,
    })
}

pub fn read_json_if_exists<T: DeserializeOwned>(path: &Path) -> Result<Option<T>, StoreError> {
    match fs::read(path) {
        Ok(encoded) => serde_json::from_slice(&encoded)
            .map(Some)
            .map_err(|source| StoreError::Decode {
                path: path.to_owned(),
                source,
            }),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(io_error(path, source)),
    }
}

pub fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<(), StoreError> {
    let mut encoded = serde_json::to_vec_pretty(value)?;
    encoded.push(b'\n');
    write_bytes_atomic(path, &encoded)
}

pub fn write_text_atomic(path: &Path, value: &str) -> Result<(), StoreError> {
    write_bytes_atomic(path, value.as_bytes())
}

pub fn write_bytes_atomic(path: &Path, value: &[u8]) -> Result<(), StoreError> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty());
    if let Some(parent) = parent {
        fs::create_dir_all(parent).map_err(|source| io_error(parent, source))?;
    }
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("snapshot");
    let temporary = path.with_file_name(format!(".{file_name}.{}.tmp", Ulid::new()));

    let result = (|| {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)
            .map_err(|source| io_error(&temporary, source))?;
        file.write_all(value)
            .map_err(|source| io_error(&temporary, source))?;
        file.sync_all()
            .map_err(|source| io_error(&temporary, source))?;
        fs::rename(&temporary, path).map_err(|source| io_error(path, source))?;
        if let Some(parent) = parent {
            sync_directory(parent)?;
        }
        Ok(())
    })();

    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

pub fn append_jsonl<T: Serialize>(path: &Path, value: &T) -> Result<(), StoreError> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty());
    if let Some(parent) = parent {
        fs::create_dir_all(parent).map_err(|source| io_error(parent, source))?;
    }
    let mut encoded = serde_json::to_vec(value)?;
    encoded.push(b'\n');
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|source| io_error(path, source))?;
    file.write_all(&encoded)
        .map_err(|source| io_error(path, source))?;
    file.sync_data().map_err(|source| io_error(path, source))
}

pub fn read_jsonl<T: DeserializeOwned>(path: &Path) -> Result<Vec<T>, StoreError> {
    let source = match fs::read_to_string(path) {
        Ok(source) => source,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => return Err(io_error(path, source)),
    };
    source
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            serde_json::from_str(line).map_err(|source| StoreError::Decode {
                path: path.to_owned(),
                source,
            })
        })
        .collect()
}

pub fn sha256_json<T: Serialize>(value: &T) -> Result<String, StoreError> {
    let encoded = serde_json::to_vec(value)?;
    Ok(sha256_bytes(&encoded))
}

#[must_use]
pub fn sha256_bytes(value: &[u8]) -> String {
    let digest = Sha256::digest(value);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub fn sha256_file(path: &Path) -> Result<String, StoreError> {
    let mut file = File::open(path).map_err(|source| io_error(path, source))?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|source| io_error(path, source))?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn sync_directory(path: &Path) -> Result<(), StoreError> {
    let directory = File::open(path).map_err(|source| io_error(path, source))?;
    directory
        .sync_all()
        .map_err(|source| io_error(path, source))
}

#[cfg(test)]
mod tests {
    use serde::{Deserialize, Serialize};

    use super::*;

    #[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
    struct Snapshot {
        revision: u64,
        name: String,
    }

    #[test]
    fn atomic_json_replaces_complete_snapshot() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state.json");
        let first = Snapshot {
            revision: 1,
            name: "first".to_owned(),
        };
        let second = Snapshot {
            revision: 2,
            name: "second".to_owned(),
        };
        write_json_atomic(&path, &first).unwrap();
        write_json_atomic(&path, &second).unwrap();

        assert_eq!(read_json::<Snapshot>(&path).unwrap(), second);
        assert_eq!(
            fs::read_dir(directory.path()).unwrap().count(),
            1,
            "temporary files must not remain"
        );
    }

    #[test]
    fn canonical_struct_hash_is_stable() {
        let snapshot = Snapshot {
            revision: 1,
            name: "same".to_owned(),
        };
        assert_eq!(
            sha256_json(&snapshot).unwrap(),
            sha256_json(&snapshot).unwrap()
        );
    }

    #[test]
    fn failed_atomic_replace_removes_temporary_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state.json");
        fs::create_dir(&path).unwrap();

        let error = write_bytes_atomic(&path, b"replacement").unwrap_err();

        assert!(matches!(error, StoreError::Io { .. }));
        assert!(path.is_dir());
        assert_eq!(
            fs::read_dir(directory.path()).unwrap().count(),
            1,
            "failed writes must not leave a temporary file"
        );
    }

    #[test]
    fn optional_json_distinguishes_missing_and_invalid_files() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("optional.json");
        assert_eq!(read_json_if_exists::<Snapshot>(&path).unwrap(), None);

        fs::write(&path, b"{invalid").unwrap();
        let error = read_json_if_exists::<Snapshot>(&path).unwrap_err();
        assert!(matches!(
            error,
            StoreError::Decode { path: ref error_path, .. } if error_path == &path
        ));
    }

    #[test]
    fn jsonl_round_trip_ignores_blank_lines() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("events/events.jsonl");
        let first = Snapshot {
            revision: 1,
            name: "first".to_owned(),
        };
        let second = Snapshot {
            revision: 2,
            name: "second".to_owned(),
        };

        append_jsonl(&path, &first).unwrap();
        fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"\n  \n")
            .unwrap();
        append_jsonl(&path, &second).unwrap();

        assert_eq!(read_jsonl::<Snapshot>(&path).unwrap(), [first, second]);
    }

    #[test]
    fn invalid_jsonl_reports_the_source_path() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("events.jsonl");
        fs::write(&path, "{\"revision\":1,\"name\":\"valid\"}\nnot-json\n").unwrap();

        let error = read_jsonl::<Snapshot>(&path).unwrap_err();

        assert!(matches!(
            error,
            StoreError::Decode { path: ref error_path, .. } if error_path == &path
        ));
    }

    #[test]
    fn file_hash_matches_in_memory_hash_for_exact_bytes() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("media.bin");
        let bytes = b"SparkStage\0binary\ncontent";
        fs::write(&path, bytes).unwrap();

        assert_eq!(sha256_file(&path).unwrap(), sha256_bytes(bytes));
    }
}
