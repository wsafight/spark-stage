use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use ulid::Ulid;

use crate::domain::{PROJECT_SCHEMA_VERSION, ProjectManifest, ProjectState};
use crate::store::{
    ExclusiveFileLock, ProjectStore, StoreError, read_json, sha256_bytes, sha256_file, sha256_json,
    validate_project_id,
};

pub const ARCHIVE_SCHEMA_VERSION: &str = "1.0";
const MAX_ARCHIVE_MANIFEST_BYTES: u64 = 16 * 1024 * 1024;
const CORE_PROJECT_FILE_LIMIT: u64 = 64 * 1024 * 1024;
const PROJECT_DIRECTORIES: &[&str] = &[
    "contracts",
    "jobs",
    "raw",
    "review",
    "builds",
    "logs",
    "trash",
];

mod migration;

pub use migration::{MigrationPlan, apply_migration, plan_migration};

#[cfg(test)]
use migration::LEGACY_PROJECT_SCHEMA_VERSION;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchiveFile {
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchiveManifest {
    pub schema_version: String,
    pub project_id: String,
    pub project_schema_version: String,
    pub files: Vec<ArchiveFile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectVerification {
    pub project_id: String,
    pub schema_version: String,
    pub revision: u64,
    pub files: usize,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchiveVerification {
    pub project_id: String,
    pub schema_version: String,
    pub project_schema_version: String,
    pub files: usize,
    pub bytes: u64,
}

pub fn verify_project(
    projects_dir: &Path,
    project_id: &str,
) -> Result<ProjectVerification, PortabilityError> {
    let store = ProjectStore::open(projects_dir, project_id)?;
    let _lock = store.lock()?;
    verify_project_locked(&store)
}

pub fn export_project(
    projects_dir: &Path,
    project_id: &str,
    destination: &Path,
) -> Result<ArchiveVerification, PortabilityError> {
    if destination.exists() {
        return Err(PortabilityError::DestinationExists(destination.to_owned()));
    }
    let store = ProjectStore::open(projects_dir, project_id)?;
    let _lock = store.lock()?;
    let verification = verify_project_locked(&store)?;
    reject_destination_inside_project(store.root(), destination)?;
    let files = collect_files(store.root())?;
    let manifest = create_archive_manifest(&verification, &files)?;
    write_archive(store.root(), destination, &manifest)?;
    Ok(archive_report(&manifest))
}

pub fn verify_archive(path: &Path) -> Result<ArchiveVerification, PortabilityError> {
    let manifest = read_archive_manifest(path)?;
    verify_archive_payload(path, &manifest)?;
    Ok(archive_report(&manifest))
}

pub fn import_project(
    projects_dir: &Path,
    archive_path: &Path,
) -> Result<ProjectVerification, PortabilityError> {
    fs::create_dir_all(projects_dir).map_err(|source| io_error(projects_dir, source))?;
    let _projects_lock = ExclusiveFileLock::acquire(&projects_dir.join(".projects.lock"))?;
    let manifest = read_archive_manifest(archive_path)?;
    verify_archive_payload(archive_path, &manifest)?;
    validate_project_id(&manifest.project_id)?;
    let target = projects_dir.join(&manifest.project_id);
    if target.exists() {
        return Err(PortabilityError::ProjectExists(manifest.project_id));
    }
    let staging = projects_dir.join(format!(".{}.import-{}", manifest.project_id, Ulid::new()));
    let staged_project = staging.join(&manifest.project_id);
    let result = (|| {
        fs::create_dir(&staging).map_err(|source| io_error(&staging, source))?;
        fs::create_dir(&staged_project).map_err(|source| io_error(&staged_project, source))?;
        extract_verified_archive(archive_path, &staged_project, &manifest)?;
        for relative in PROJECT_DIRECTORIES {
            let directory = staged_project.join(relative);
            fs::create_dir_all(&directory).map_err(|source| io_error(&directory, source))?;
        }
        verify_project(&staging, &manifest.project_id)?;
        if target.exists() {
            return Err(PortabilityError::ProjectExists(manifest.project_id.clone()));
        }
        fs::rename(&staged_project, &target).map_err(|source| io_error(&target, source))?;
        fs::remove_dir(&staging).map_err(|source| io_error(&staging, source))?;
        sync_directory(projects_dir)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&staging);
    }
    result?;
    verify_project(projects_dir, &manifest.project_id)
}

fn verify_project_locked(store: &ProjectStore) -> Result<ProjectVerification, PortabilityError> {
    let manifest = store.read_manifest()?;
    if manifest.schema_version != PROJECT_SCHEMA_VERSION {
        return Err(PortabilityError::SchemaMismatch {
            subject: "project.json",
            actual: manifest.schema_version,
        });
    }
    let state = store.read_state()?;
    if state.project_id != manifest.project_id || state.title != manifest.title {
        return Err(PortabilityError::ProjectIdentityMismatch);
    }
    let brief_path = store.root().join("script/brief.md");
    let brief = fs::read(&brief_path).map_err(|source| io_error(&brief_path, source))?;
    if sha256_bytes(&brief) != manifest.brief_hash {
        return Err(PortabilityError::HashMismatch("script/brief.md".to_owned()));
    }
    for (contract_id, record) in &state.contracts {
        validate_relative_path(&record.relative_path)?;
        let bundle = store.read_contract_bundle_by_id(contract_id)?;
        if sha256_json(&bundle)? != record.bundle_hash {
            return Err(PortabilityError::HashMismatch(format!(
                "contract {contract_id}"
            )));
        }
    }
    let files = collect_files(store.root())?;
    let bytes = files.iter().try_fold(0_u64, |total, file| {
        total
            .checked_add(file.bytes)
            .ok_or(PortabilityError::SizeOverflow)
    })?;
    Ok(ProjectVerification {
        project_id: manifest.project_id,
        schema_version: manifest.schema_version,
        revision: state.revision,
        files: files.len(),
        bytes,
    })
}

fn create_archive_manifest(
    verification: &ProjectVerification,
    files: &[CollectedFile],
) -> Result<ArchiveManifest, PortabilityError> {
    Ok(ArchiveManifest {
        schema_version: ARCHIVE_SCHEMA_VERSION.to_owned(),
        project_id: verification.project_id.clone(),
        project_schema_version: verification.schema_version.clone(),
        files: files
            .iter()
            .map(|file| {
                Ok(ArchiveFile {
                    path: relative_string(&file.relative)?,
                    bytes: file.bytes,
                    sha256: file.sha256.clone(),
                })
            })
            .collect::<Result<_, PortabilityError>>()?,
    })
}

fn write_archive(
    project_root: &Path,
    destination: &Path,
    manifest: &ArchiveManifest,
) -> Result<(), PortabilityError> {
    let parent = destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|source| io_error(parent, source))?;
    let file_name = destination
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("project.sparkstage.tar");
    let staging = destination.with_file_name(format!(".{file_name}.{}.tmp", Ulid::new()));
    let result = (|| {
        let output = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&staging)
            .map_err(|source| io_error(&staging, source))?;
        let mut archive = tar::Builder::new(output);
        let mut encoded = serde_json::to_vec_pretty(manifest)?;
        encoded.push(b'\n');
        append_bytes(&mut archive, "archive-manifest.json", &encoded)?;
        for file in &manifest.files {
            let source = project_root.join(&file.path);
            let mut input = File::open(&source).map_err(|error| io_error(&source, error))?;
            let mut header = tar_header(file.bytes);
            archive
                .append_data(&mut header, format!("project/{}", file.path), &mut input)
                .map_err(|error| PortabilityError::Archive(error.to_string()))?;
        }
        let output = archive
            .into_inner()
            .map_err(|error| PortabilityError::Archive(error.to_string()))?;
        output
            .sync_all()
            .map_err(|source| io_error(&staging, source))?;
        fs::rename(&staging, destination).map_err(|source| io_error(destination, source))?;
        sync_directory(parent)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&staging);
    }
    result
}

fn append_bytes(
    archive: &mut tar::Builder<File>,
    path: &str,
    bytes: &[u8],
) -> Result<(), PortabilityError> {
    let size = u64::try_from(bytes.len()).map_err(|_| PortabilityError::SizeOverflow)?;
    let mut header = tar_header(size);
    archive
        .append_data(&mut header, path, bytes)
        .map_err(|error| PortabilityError::Archive(error.to_string()))
}

fn tar_header(size: u64) -> tar::Header {
    let mut header = tar::Header::new_gnu();
    header.set_size(size);
    header.set_mode(0o644);
    header.set_uid(0);
    header.set_gid(0);
    header.set_mtime(0);
    header.set_entry_type(tar::EntryType::Regular);
    header.set_cksum();
    header
}

fn read_archive_manifest(path: &Path) -> Result<ArchiveManifest, PortabilityError> {
    let file = File::open(path).map_err(|source| io_error(path, source))?;
    let mut archive = tar::Archive::new(file);
    let mut entries = archive
        .entries()
        .map_err(|error| PortabilityError::Archive(error.to_string()))?;
    let mut entry = entries
        .next()
        .ok_or_else(|| PortabilityError::Archive("archive is empty".to_owned()))?
        .map_err(|error| PortabilityError::Archive(error.to_string()))?;
    let entry_path = entry
        .path()
        .map_err(|error| PortabilityError::Archive(error.to_string()))?;
    if entry_path.as_ref() != Path::new("archive-manifest.json")
        || !entry.header().entry_type().is_file()
    {
        return Err(PortabilityError::Archive(
            "the first entry must be archive-manifest.json".to_owned(),
        ));
    }
    if entry.size() > MAX_ARCHIVE_MANIFEST_BYTES {
        return Err(PortabilityError::Archive(
            "archive manifest exceeds the size limit".to_owned(),
        ));
    }
    let mut encoded = Vec::new();
    entry
        .read_to_end(&mut encoded)
        .map_err(|error| PortabilityError::Archive(error.to_string()))?;
    let manifest: ArchiveManifest = serde_json::from_slice(&encoded)?;
    validate_archive_manifest(&manifest)?;
    Ok(manifest)
}

fn validate_archive_manifest(manifest: &ArchiveManifest) -> Result<(), PortabilityError> {
    if manifest.schema_version != ARCHIVE_SCHEMA_VERSION {
        return Err(PortabilityError::SchemaMismatch {
            subject: "archive manifest",
            actual: manifest.schema_version.clone(),
        });
    }
    validate_project_id(&manifest.project_id)?;
    if manifest.project_schema_version != PROJECT_SCHEMA_VERSION {
        return Err(PortabilityError::SchemaMismatch {
            subject: "archived project",
            actual: manifest.project_schema_version.clone(),
        });
    }
    let mut previous: Option<&str> = None;
    let mut paths = BTreeSet::new();
    for file in &manifest.files {
        validate_archive_relative(&file.path)?;
        if file.sha256.len() != 64 || !file.sha256.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(PortabilityError::Archive(format!(
                "invalid SHA-256 for {}",
                file.path
            )));
        }
        if previous.is_some_and(|value| value >= file.path.as_str())
            || !paths.insert(file.path.as_str())
        {
            return Err(PortabilityError::Archive(
                "archive file list must be sorted and unique".to_owned(),
            ));
        }
        previous = Some(&file.path);
    }
    for required in ["project.json", "script/brief.md", "state.json"] {
        if !paths.contains(required) {
            return Err(PortabilityError::Archive(format!(
                "archive is missing {required}"
            )));
        }
    }
    Ok(())
}

fn verify_archive_payload(
    archive_path: &Path,
    manifest: &ArchiveManifest,
) -> Result<(), PortabilityError> {
    let expected = manifest
        .files
        .iter()
        .map(|file| (file.path.as_str(), file))
        .collect::<BTreeMap<_, _>>();
    let file = File::open(archive_path).map_err(|source| io_error(archive_path, source))?;
    let mut archive = tar::Archive::new(file);
    let mut seen = BTreeSet::new();
    let mut project_json = None;
    let mut state_json = None;
    let mut brief = None;
    for (index, entry) in archive
        .entries()
        .map_err(|error| PortabilityError::Archive(error.to_string()))?
        .enumerate()
    {
        let mut entry = entry.map_err(|error| PortabilityError::Archive(error.to_string()))?;
        if index == 0 {
            continue;
        }
        if !entry.header().entry_type().is_file() {
            return Err(PortabilityError::Archive(
                "archive payload may contain only regular files".to_owned(),
            ));
        }
        let relative = archive_payload_path(&entry)?;
        let expected_file = expected
            .get(relative.as_str())
            .ok_or_else(|| PortabilityError::UnexpectedArchiveFile(relative.clone()))?;
        if !seen.insert(relative.clone()) {
            return Err(PortabilityError::UnexpectedArchiveFile(relative));
        }
        let keep = matches!(
            relative.as_str(),
            "project.json" | "script/brief.md" | "state.json"
        );
        let (bytes, hash, content) = hash_reader(&mut entry, keep)?;
        if bytes != expected_file.bytes || hash != expected_file.sha256 {
            return Err(PortabilityError::HashMismatch(relative));
        }
        match relative.as_str() {
            "project.json" => project_json = content,
            "script/brief.md" => brief = content,
            "state.json" => state_json = content,
            _ => {}
        }
    }
    if seen.len() != expected.len() {
        let missing = expected
            .keys()
            .find(|path| !seen.contains(**path))
            .copied()
            .unwrap_or("unknown");
        return Err(PortabilityError::MissingArchiveFile(missing.to_owned()));
    }
    let state = validate_core_project_files(
        project_json
            .ok_or_else(|| PortabilityError::MissingArchiveFile("project.json".to_owned()))?,
        state_json.ok_or_else(|| PortabilityError::MissingArchiveFile("state.json".to_owned()))?,
        brief.ok_or_else(|| PortabilityError::MissingArchiveFile("script/brief.md".to_owned()))?,
        &manifest.project_id,
    )?;
    validate_archived_contracts(archive_path, &state)
}

fn extract_verified_archive(
    archive_path: &Path,
    staging: &Path,
    manifest: &ArchiveManifest,
) -> Result<(), PortabilityError> {
    let expected = manifest
        .files
        .iter()
        .map(|file| (file.path.as_str(), file))
        .collect::<BTreeMap<_, _>>();
    let file = File::open(archive_path).map_err(|source| io_error(archive_path, source))?;
    let mut archive = tar::Archive::new(file);
    let mut seen = BTreeSet::new();
    for (index, entry) in archive
        .entries()
        .map_err(|error| PortabilityError::Archive(error.to_string()))?
        .enumerate()
    {
        let mut entry = entry.map_err(|error| PortabilityError::Archive(error.to_string()))?;
        if index == 0 {
            continue;
        }
        let relative = archive_payload_path(&entry)?;
        let expected_file = expected
            .get(relative.as_str())
            .ok_or_else(|| PortabilityError::UnexpectedArchiveFile(relative.clone()))?;
        if !seen.insert(relative.clone()) {
            return Err(PortabilityError::UnexpectedArchiveFile(relative));
        }
        let output_path = staging.join(&relative);
        let parent = output_path
            .parent()
            .ok_or_else(|| PortabilityError::UnsafePath(relative.clone()))?;
        fs::create_dir_all(parent).map_err(|source| io_error(parent, source))?;
        let mut output = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&output_path)
            .map_err(|source| io_error(&output_path, source))?;
        let (bytes, hash) = copy_and_hash(&mut entry, &mut output)?;
        output
            .sync_all()
            .map_err(|source| io_error(&output_path, source))?;
        if bytes != expected_file.bytes || hash != expected_file.sha256 {
            return Err(PortabilityError::HashMismatch(relative));
        }
    }
    if seen.len() != expected.len() {
        return Err(PortabilityError::Archive(
            "archive payload is incomplete".to_owned(),
        ));
    }
    Ok(())
}

fn archive_payload_path<R: Read>(entry: &tar::Entry<'_, R>) -> Result<String, PortabilityError> {
    let path = entry
        .path()
        .map_err(|error| PortabilityError::Archive(error.to_string()))?;
    let relative = path
        .strip_prefix("project")
        .map_err(|_| PortabilityError::UnsafePath(path.display().to_string()))?;
    validate_relative_path(relative)?;
    relative_string(relative)
}

fn hash_reader<R: Read>(
    reader: &mut R,
    retain: bool,
) -> Result<(u64, String, Option<Vec<u8>>), PortabilityError> {
    let mut digest = Sha256::new();
    let mut bytes = 0_u64;
    let mut content = retain.then(Vec::new);
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|error| PortabilityError::Archive(error.to_string()))?;
        if read == 0 {
            break;
        }
        bytes = bytes
            .checked_add(u64::try_from(read).map_err(|_| PortabilityError::SizeOverflow)?)
            .ok_or(PortabilityError::SizeOverflow)?;
        if retain && bytes > CORE_PROJECT_FILE_LIMIT {
            return Err(PortabilityError::Archive(
                "core project file exceeds the size limit".to_owned(),
            ));
        }
        digest.update(&buffer[..read]);
        if let Some(content) = &mut content {
            content.extend_from_slice(&buffer[..read]);
        }
    }
    Ok((bytes, hex_digest(digest), content))
}

fn copy_and_hash<R: Read, W: Write>(
    reader: &mut R,
    writer: &mut W,
) -> Result<(u64, String), PortabilityError> {
    let mut digest = Sha256::new();
    let mut bytes = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|error| PortabilityError::Archive(error.to_string()))?;
        if read == 0 {
            break;
        }
        writer
            .write_all(&buffer[..read])
            .map_err(|error| PortabilityError::Archive(error.to_string()))?;
        digest.update(&buffer[..read]);
        bytes = bytes
            .checked_add(u64::try_from(read).map_err(|_| PortabilityError::SizeOverflow)?)
            .ok_or(PortabilityError::SizeOverflow)?;
    }
    Ok((bytes, hex_digest(digest)))
}

fn validate_core_project_files(
    manifest_bytes: Vec<u8>,
    state_bytes: Vec<u8>,
    brief_bytes: Vec<u8>,
    project_id: &str,
) -> Result<ProjectState, PortabilityError> {
    let manifest: ProjectManifest = serde_json::from_slice(&manifest_bytes)?;
    let state: ProjectState = serde_json::from_slice(&state_bytes)?;
    state.validate()?;
    if manifest.schema_version != PROJECT_SCHEMA_VERSION
        || state.schema_version != PROJECT_SCHEMA_VERSION
    {
        return Err(PortabilityError::SchemaMismatch {
            subject: "project core files",
            actual: format!("{}/{}", manifest.schema_version, state.schema_version),
        });
    }
    if manifest.project_id != project_id || state.project_id != project_id {
        return Err(PortabilityError::ProjectIdentityMismatch);
    }
    if sha256_bytes(&brief_bytes) != manifest.brief_hash {
        return Err(PortabilityError::HashMismatch("script/brief.md".to_owned()));
    }
    Ok(state)
}

fn validate_archived_contracts(
    archive_path: &Path,
    state: &ProjectState,
) -> Result<(), PortabilityError> {
    let mut expected = BTreeMap::new();
    for (contract_id, record) in &state.contracts {
        validate_relative_path(&record.relative_path)?;
        let path = record.relative_path.join("script/bundle.json");
        expected.insert(relative_string(&path)?, (contract_id, &record.bundle_hash));
    }
    if expected.is_empty() {
        return Ok(());
    }
    let file = File::open(archive_path).map_err(|source| io_error(archive_path, source))?;
    let mut archive = tar::Archive::new(file);
    let mut seen = BTreeSet::new();
    for (index, entry) in archive
        .entries()
        .map_err(|error| PortabilityError::Archive(error.to_string()))?
        .enumerate()
    {
        let mut entry = entry.map_err(|error| PortabilityError::Archive(error.to_string()))?;
        if index == 0 {
            continue;
        }
        let relative = archive_payload_path(&entry)?;
        let Some((contract_id, expected_hash)) = expected.get(&relative) else {
            continue;
        };
        let (_, _, content) = hash_reader(&mut entry, true)?;
        let bundle: crate::domain::ScriptBundle = serde_json::from_slice(
            &content.ok_or_else(|| PortabilityError::MissingArchiveFile(relative.clone()))?,
        )?;
        if sha256_json(&bundle)? != **expected_hash {
            return Err(PortabilityError::HashMismatch(format!(
                "contract {contract_id}"
            )));
        }
        seen.insert(relative);
    }
    if seen.len() != expected.len() {
        let missing = expected
            .keys()
            .find(|path| !seen.contains(*path))
            .cloned()
            .unwrap_or_else(|| "contract bundle".to_owned());
        return Err(PortabilityError::MissingArchiveFile(missing));
    }
    Ok(())
}

fn verify_extracted_project(root: &Path, project_id: &str) -> Result<(), PortabilityError> {
    let manifest: ProjectManifest = read_json(&root.join("project.json"))?;
    let state: ProjectState = read_json(&root.join("state.json"))?;
    state.validate()?;
    if manifest.schema_version != PROJECT_SCHEMA_VERSION
        || manifest.project_id != project_id
        || state.project_id != project_id
        || state.title != manifest.title
    {
        return Err(PortabilityError::ProjectIdentityMismatch);
    }
    Ok(())
}

#[derive(Debug)]
struct CollectedFile {
    relative: PathBuf,
    bytes: u64,
    sha256: String,
}

fn collect_files(root: &Path) -> Result<Vec<CollectedFile>, PortabilityError> {
    let mut paths = Vec::new();
    collect_file_paths(root, root, &mut paths)?;
    paths.sort();
    paths
        .into_iter()
        .map(|relative| {
            let path = root.join(&relative);
            let bytes = fs::metadata(&path)
                .map_err(|source| io_error(&path, source))?
                .len();
            Ok(CollectedFile {
                sha256: sha256_file(&path)?,
                relative,
                bytes,
            })
        })
        .collect()
}

fn collect_file_paths(
    root: &Path,
    directory: &Path,
    paths: &mut Vec<PathBuf>,
) -> Result<(), PortabilityError> {
    let entries = fs::read_dir(directory).map_err(|source| io_error(directory, source))?;
    for entry in entries {
        let entry = entry.map_err(|source| io_error(directory, source))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|source| io_error(&path, source))?;
        if file_type.is_symlink() {
            return Err(PortabilityError::Symlink(path));
        }
        if file_type.is_dir() {
            collect_file_paths(root, &path, paths)?;
        } else if file_type.is_file() {
            let relative = path
                .strip_prefix(root)
                .map_err(|_| PortabilityError::UnsafePath(path.display().to_string()))?;
            if relative != Path::new("project.lock") {
                validate_relative_path(relative)?;
                paths.push(relative.to_owned());
            }
        } else {
            return Err(PortabilityError::UnsupportedFile(path));
        }
    }
    Ok(())
}

fn reject_destination_inside_project(
    project_root: &Path,
    destination: &Path,
) -> Result<(), PortabilityError> {
    let project =
        fs::canonicalize(project_root).map_err(|source| io_error(project_root, source))?;
    let parent = destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|source| io_error(parent, source))?;
    let parent = fs::canonicalize(parent).map_err(|source| io_error(parent, source))?;
    let candidate = parent.join(
        destination
            .file_name()
            .ok_or_else(|| PortabilityError::UnsafePath(destination.display().to_string()))?,
    );
    if candidate.starts_with(project) {
        Err(PortabilityError::DestinationInsideProject(
            destination.to_owned(),
        ))
    } else {
        Ok(())
    }
}

fn validate_archive_relative(value: &str) -> Result<(), PortabilityError> {
    if value.contains('\\') {
        return Err(PortabilityError::UnsafePath(value.to_owned()));
    }
    validate_relative_path(Path::new(value))
}

fn validate_relative_path(path: &Path) -> Result<(), PortabilityError> {
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        Err(PortabilityError::UnsafePath(path.display().to_string()))
    } else {
        Ok(())
    }
}

fn relative_string(path: &Path) -> Result<String, PortabilityError> {
    validate_relative_path(path)?;
    path.components()
        .map(|component| match component {
            Component::Normal(value) => value
                .to_str()
                .map(str::to_owned)
                .ok_or_else(|| PortabilityError::NonUtf8Path(path.to_owned())),
            _ => Err(PortabilityError::UnsafePath(path.display().to_string())),
        })
        .collect::<Result<Vec<_>, _>>()
        .map(|parts| parts.join("/"))
}

fn archive_report(manifest: &ArchiveManifest) -> ArchiveVerification {
    ArchiveVerification {
        project_id: manifest.project_id.clone(),
        schema_version: manifest.schema_version.clone(),
        project_schema_version: manifest.project_schema_version.clone(),
        files: manifest.files.len(),
        bytes: manifest.files.iter().map(|file| file.bytes).sum(),
    }
}

fn hex_digest(digest: Sha256) -> String {
    digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn sync_directory(path: &Path) -> Result<(), PortabilityError> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|source| io_error(path, source))
}

fn io_error(path: &Path, source: std::io::Error) -> PortabilityError {
    PortabilityError::Io {
        path: path.to_owned(),
        source,
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PortabilityError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    State(#[from] crate::domain::StateInvariantError),
    #[error("cannot access `{path}`: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("cannot encode or decode portability JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid archive: {0}")]
    Archive(String),
    #[error("project `{0}` does not exist")]
    ProjectNotFound(String),
    #[error("project `{0}` already exists; import never overwrites projects")]
    ProjectExists(String),
    #[error("destination `{0}` already exists")]
    DestinationExists(PathBuf),
    #[error("archive destination `{0}` is inside the source project")]
    DestinationInsideProject(PathBuf),
    #[error("unsafe relative path `{0}`")]
    UnsafePath(String),
    #[error("project contains symlink `{0}`")]
    Symlink(PathBuf),
    #[error("project contains unsupported file type `{0}`")]
    UnsupportedFile(PathBuf),
    #[error("project path is not valid UTF-8: `{0}`")]
    NonUtf8Path(PathBuf),
    #[error("SHA-256 mismatch for `{0}`")]
    HashMismatch(String),
    #[error("archive contains unexpected or duplicate file `{0}`")]
    UnexpectedArchiveFile(String),
    #[error("archive is missing `{0}`")]
    MissingArchiveFile(String),
    #[error("schema version is missing from `{0}`")]
    SchemaMissing(&'static str),
    #[error("unsupported schema `{actual}` in {subject}")]
    SchemaMismatch {
        subject: &'static str,
        actual: String,
    },
    #[error("project manifest and state identity do not match")]
    ProjectIdentityMismatch,
    #[error("migration is unsupported for project/state schemas {project}/{state}")]
    MigrationUnsupported { project: String, state: String },
    #[error("project or archive size overflow")]
    SizeOverflow,
}

#[cfg(test)]
mod tests;
