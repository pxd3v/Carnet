use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
};

#[cfg(unix)]
use cap_std::fs::MetadataExt;
use cap_std::fs::{Dir, File, OpenOptions, Permissions};
use sha2::{Digest, Sha256};
use thiserror::Error;

use super::{NotePath, PathError, Workspace, paths};

#[cfg(test)]
thread_local! {
    static SNAPSHOT_READ_COUNT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

const UTF8_BOM: &[u8] = b"\xef\xbb\xbf";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NewlineStyle {
    Lf,
    CrLf,
}

#[derive(Clone, Debug)]
pub struct LoadedNote {
    path: NotePath,
    text: String,
    hash: Option<[u8; 32]>,
    has_bom: bool,
    newline_style: NewlineStyle,
    had_final_newline: bool,
    permissions: Option<Permissions>,
    fingerprint: Option<FileFingerprint>,
}

#[cfg(unix)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FileFingerprint {
    device: u64,
    inode: u64,
    size: u64,
    modified_seconds: i64,
    modified_nanoseconds: i64,
    changed_seconds: i64,
    changed_nanoseconds: i64,
}

#[cfg(not(unix))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FileFingerprint;

#[derive(Clone, Debug)]
pub enum FileOperation {
    Save {
        note: LoadedNote,
        content: String,
        overwrite: bool,
    },
    CreateFile {
        workspace: Workspace,
        path: PathBuf,
    },
    CreateFolder {
        workspace: Workspace,
        path: PathBuf,
    },
    Rename {
        workspace: Workspace,
        from: PathBuf,
        to: PathBuf,
    },
    Move {
        workspace: Workspace,
        from: PathBuf,
        to: PathBuf,
    },
    Delete {
        workspace: Workspace,
        path: PathBuf,
        confirmed: bool,
    },
}

#[derive(Clone, Debug)]
pub enum FileOutcome {
    Saved(LoadedNote),
    CreatedFile(NotePath),
    CreatedFolder(PathBuf),
    Renamed { from: PathBuf, to: PathBuf },
    Moved { from: PathBuf, to: PathBuf },
    Deleted(PathBuf),
}

impl LoadedNote {
    pub fn path(&self) -> &NotePath {
        &self.path
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn content_hash(&self) -> Option<[u8; 32]> {
        self.hash
    }

    pub fn has_bom(&self) -> bool {
        self.has_bom
    }

    pub fn newline_style(&self) -> NewlineStyle {
        self.newline_style
    }

    pub fn had_final_newline(&self) -> bool {
        self.had_final_newline
    }

    pub fn is_saved(&self) -> bool {
        self.hash.is_some()
    }
}

#[derive(Debug, Error)]
pub enum FileError {
    #[error(transparent)]
    Path(#[from] PathError),
    #[error("could not access file at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("file is not valid UTF-8: {path}")]
    InvalidUtf8 { path: PathBuf },
    #[error("file contains binary data: {path}")]
    Binary { path: PathBuf },
    #[error("Git ignore check failed: {message}")]
    GitIgnore { message: String },
    #[error("note was modified outside Carnet: {path}")]
    ExternalModification { path: PathBuf },
    #[error("note was deleted outside Carnet: {path}")]
    ExternalDeletion { path: PathBuf },
    #[error("path already exists: {path}")]
    AlreadyExists { path: PathBuf },
    #[error("path does not exist: {path}")]
    Missing { path: PathBuf },
    #[error("deletion requires confirmation: {path}")]
    ConfirmationRequired { path: PathBuf },
}

pub(crate) fn bytes_are_text(bytes: &[u8]) -> bool {
    !bytes.contains(&0)
        && std::str::from_utf8(bytes.strip_prefix(UTF8_BOM).unwrap_or(bytes)).is_ok()
}

pub(crate) fn load_note(path: &NotePath) -> Result<LoadedNote, FileError> {
    paths::revalidate_note(path)?;
    let snapshot = match read_snapshot(path.directory(), path.relative()) {
        Ok(snapshot) => snapshot,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            return Ok(LoadedNote {
                path: path.clone(),
                text: String::new(),
                hash: None,
                has_bom: false,
                newline_style: NewlineStyle::Lf,
                had_final_newline: false,
                permissions: None,
                fingerprint: None,
            });
        }
        Err(source) => {
            return Err(FileError::Io {
                path: path.relative().to_path_buf(),
                source,
            });
        }
    };
    let bytes = snapshot.bytes;
    if bytes.contains(&0) {
        return Err(FileError::Binary {
            path: path.relative().to_path_buf(),
        });
    }
    let hash = Sha256::digest(&bytes).into();
    let has_bom = bytes.starts_with(UTF8_BOM);
    let payload = if has_bom {
        &bytes[UTF8_BOM.len()..]
    } else {
        &bytes
    };
    let decoded = std::str::from_utf8(payload).map_err(|_| FileError::InvalidUtf8 {
        path: path.relative().to_path_buf(),
    })?;
    let newline_style = if decoded.contains("\r\n") {
        NewlineStyle::CrLf
    } else {
        NewlineStyle::Lf
    };
    let had_final_newline = decoded.ends_with('\n');
    let text = match newline_style {
        NewlineStyle::Lf => decoded.to_owned(),
        NewlineStyle::CrLf => decoded.replace("\r\n", "\n"),
    };
    Ok(LoadedNote {
        path: path.clone(),
        text,
        hash: Some(hash),
        has_bom,
        newline_style,
        had_final_newline,
        permissions: Some(snapshot.permissions),
        fingerprint: Some(snapshot.fingerprint),
    })
}

pub(crate) fn apply(operation: FileOperation) -> Result<FileOutcome, FileError> {
    match operation {
        FileOperation::Save {
            note,
            content,
            overwrite,
        } => save_note(note, content, overwrite),
        FileOperation::CreateFile { workspace, path } => create_file(&workspace, &path),
        FileOperation::CreateFolder { workspace, path } => create_folder(&workspace, &path),
        FileOperation::Rename {
            workspace,
            from,
            to,
        } => relocate(&workspace, &from, &to, false),
        FileOperation::Move {
            workspace,
            from,
            to,
        } => relocate(&workspace, &from, &to, true),
        FileOperation::Delete {
            workspace,
            path,
            confirmed,
        } => delete(&workspace, &path, confirmed),
    }
}

fn create_file(workspace: &Workspace, path: &Path) -> Result<FileOutcome, FileError> {
    let directory = workspace.directory();
    let resolved = paths::resolve_target(directory, path, false)?;
    if resolved.metadata.is_some() {
        return Err(FileError::AlreadyExists {
            path: resolved.relative,
        });
    }
    create_parent_directories(directory, &resolved.relative)?;
    paths::resolve_target(directory, path, false)?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    let file = directory
        .open_with(&resolved.relative, &options)
        .map_err(|source| FileError::Io {
            path: resolved.relative.clone(),
            source,
        })?;
    file.sync_all().map_err(|source| FileError::Io {
        path: resolved.relative.clone(),
        source,
    })?;
    Ok(FileOutcome::CreatedFile(paths::resolve_note(
        workspace.root(),
        Arc::clone(directory),
        path,
    )?))
}

fn create_folder(workspace: &Workspace, path: &Path) -> Result<FileOutcome, FileError> {
    let directory = workspace.directory();
    let resolved = paths::resolve_target(directory, path, true)?;
    if resolved.metadata.is_some() {
        return Err(FileError::AlreadyExists {
            path: resolved.relative,
        });
    }
    directory
        .create_dir_all(&resolved.relative)
        .map_err(|source| FileError::Io {
            path: resolved.relative.clone(),
            source,
        })?;
    paths::resolve_target(directory, path, true)?;
    Ok(FileOutcome::CreatedFolder(resolved.relative))
}

fn relocate(
    workspace: &Workspace,
    from: &Path,
    to: &Path,
    is_move: bool,
) -> Result<FileOutcome, FileError> {
    let directory = workspace.directory();
    let source = paths::resolve_target(directory, from, true)?;
    if source.metadata.is_none() {
        return Err(FileError::Missing {
            path: source.relative,
        });
    }
    let destination = paths::resolve_target(directory, to, true)?;
    if destination.metadata.is_some() {
        return Err(FileError::AlreadyExists {
            path: destination.relative,
        });
    }
    create_parent_directories(directory, &destination.relative)?;
    let source = paths::resolve_target(directory, from, true)?;
    let destination = paths::resolve_target(directory, to, true)?;
    directory
        .rename(&source.relative, directory, &destination.relative)
        .map_err(|source| FileError::Io {
            path: destination.relative.clone(),
            source,
        })?;
    if is_move {
        Ok(FileOutcome::Moved {
            from: source.relative,
            to: destination.relative,
        })
    } else {
        Ok(FileOutcome::Renamed {
            from: source.relative,
            to: destination.relative,
        })
    }
}

fn delete(workspace: &Workspace, path: &Path, confirmed: bool) -> Result<FileOutcome, FileError> {
    if !confirmed {
        return Err(FileError::ConfirmationRequired {
            path: path.to_path_buf(),
        });
    }
    let directory = workspace.directory();
    let resolved = paths::resolve_target(directory, path, true)?;
    let metadata = resolved.metadata.ok_or_else(|| FileError::Missing {
        path: resolved.relative.clone(),
    })?;
    let result = if metadata.is_dir() {
        directory.remove_dir_all(&resolved.relative)
    } else {
        directory.remove_file(&resolved.relative)
    };
    result.map_err(|source| FileError::Io {
        path: resolved.relative.clone(),
        source,
    })?;
    Ok(FileOutcome::Deleted(resolved.relative))
}

fn save_note(note: LoadedNote, content: String, overwrite: bool) -> Result<FileOutcome, FileError> {
    save_note_before_commit(note, content, overwrite, || {})
}

fn save_note_before_commit(
    note: LoadedNote,
    content: String,
    overwrite: bool,
    before_commit: impl FnOnce(),
) -> Result<FileOutcome, FileError> {
    paths::revalidate_note(&note.path)?;
    if !overwrite {
        verify_unchanged(&note)?;
    }
    create_parent_directories(note.path.directory(), note.path.relative())?;
    paths::revalidate_note(&note.path)?;

    let bytes = encode(&note, &content);
    let mut temporary = CapabilityTempFile::new(
        Arc::clone(note.path.directory()),
        note.path.relative().parent(),
    )?;
    temporary
        .file_mut()
        .write_all(&bytes)
        .map_err(|source| FileError::Io {
            path: temporary.relative().to_path_buf(),
            source,
        })?;
    if let Some(permissions) = note.permissions.clone() {
        temporary
            .file()
            .set_permissions(permissions)
            .map_err(|source| FileError::Io {
                path: temporary.relative().to_path_buf(),
                source,
            })?;
    }
    temporary
        .file_mut()
        .flush()
        .map_err(|source| FileError::Io {
            path: temporary.relative().to_path_buf(),
            source,
        })?;
    temporary
        .file()
        .sync_all()
        .map_err(|source| FileError::Io {
            path: temporary.relative().to_path_buf(),
            source,
        })?;
    before_commit();
    if !overwrite {
        verify_fingerprint(&note)?;
    }
    temporary.persist(note.path.relative())?;
    Ok(FileOutcome::Saved(load_note(&note.path)?))
}

fn verify_unchanged(note: &LoadedNote) -> Result<(), FileError> {
    match (
        &note.hash,
        read_snapshot(note.path.directory(), note.path.relative()),
    ) {
        (Some(_), Err(source)) if source.kind() == std::io::ErrorKind::NotFound => {
            Err(FileError::ExternalDeletion {
                path: note.path.relative().to_path_buf(),
            })
        }
        (None, Err(source)) if source.kind() == std::io::ErrorKind::NotFound => Ok(()),
        (Some(expected), Ok(snapshot))
            if <[u8; 32]>::from(Sha256::digest(&snapshot.bytes)) == *expected
                && note.fingerprint == Some(snapshot.fingerprint) =>
        {
            Ok(())
        }
        (Some(_), Ok(_)) | (None, Ok(_)) => Err(FileError::ExternalModification {
            path: note.path.relative().to_path_buf(),
        }),
        (_, Err(source)) => Err(FileError::Io {
            path: note.path.relative().to_path_buf(),
            source,
        }),
    }
}

fn verify_fingerprint(note: &LoadedNote) -> Result<(), FileError> {
    match (
        note.fingerprint,
        note.path.directory().symlink_metadata(note.path.relative()),
    ) {
        (Some(_), Err(source)) if source.kind() == std::io::ErrorKind::NotFound => {
            Err(FileError::ExternalDeletion {
                path: note.path.relative().to_path_buf(),
            })
        }
        (None, Err(source)) if source.kind() == std::io::ErrorKind::NotFound => Ok(()),
        (Some(expected), Ok(metadata)) if file_fingerprint(&metadata) == expected => Ok(()),
        (Some(_), Ok(_)) | (None, Ok(_)) => Err(FileError::ExternalModification {
            path: note.path.relative().to_path_buf(),
        }),
        (_, Err(source)) => Err(FileError::Io {
            path: note.path.relative().to_path_buf(),
            source,
        }),
    }
}

struct FileSnapshot {
    bytes: Vec<u8>,
    permissions: Permissions,
    fingerprint: FileFingerprint,
}

fn read_snapshot(directory: &Dir, relative: &Path) -> std::io::Result<FileSnapshot> {
    #[cfg(test)]
    SNAPSHOT_READ_COUNT.with(|count| count.set(count.get() + 1));
    let mut file = directory.open(relative)?;
    let metadata = file.metadata()?;
    let fingerprint = file_fingerprint(&metadata);
    let permissions = metadata.permissions();
    let mut bytes = Vec::with_capacity(metadata.len().try_into().unwrap_or(0));
    file.read_to_end(&mut bytes)?;
    Ok(FileSnapshot {
        bytes,
        permissions,
        fingerprint,
    })
}

#[cfg(test)]
fn reset_snapshot_read_count() {
    SNAPSHOT_READ_COUNT.with(|count| count.set(0));
}

#[cfg(test)]
fn snapshot_read_count() -> usize {
    SNAPSHOT_READ_COUNT.with(std::cell::Cell::get)
}

#[cfg(unix)]
fn file_fingerprint(metadata: &cap_std::fs::Metadata) -> FileFingerprint {
    FileFingerprint {
        device: metadata.dev(),
        inode: metadata.ino(),
        size: metadata.len(),
        modified_seconds: metadata.mtime(),
        modified_nanoseconds: metadata.mtime_nsec(),
        changed_seconds: metadata.ctime(),
        changed_nanoseconds: metadata.ctime_nsec(),
    }
}

#[cfg(not(unix))]
fn file_fingerprint(_metadata: &cap_std::fs::Metadata) -> FileFingerprint {
    FileFingerprint
}

fn create_parent_directories(directory: &Dir, relative: &Path) -> Result<(), FileError> {
    let Some(parent) = relative
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    else {
        return Ok(());
    };
    directory
        .create_dir_all(parent)
        .map_err(|source| FileError::Io {
            path: parent.to_path_buf(),
            source,
        })
}

struct CapabilityTempFile {
    directory: Arc<Dir>,
    relative: PathBuf,
    file: File,
    persisted: bool,
}

impl CapabilityTempFile {
    fn new(directory: Arc<Dir>, parent: Option<&Path>) -> Result<Self, FileError> {
        let parent = parent.filter(|parent| !parent.as_os_str().is_empty());
        loop {
            let name = format!(".carnet-{}.tmp", uuid::Uuid::new_v4());
            let relative = parent.map_or_else(|| PathBuf::from(&name), |parent| parent.join(&name));
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            match directory.open_with(&relative, &options) {
                Ok(file) => {
                    return Ok(Self {
                        directory,
                        relative,
                        file,
                        persisted: false,
                    });
                }
                Err(source) if source.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(source) => {
                    return Err(FileError::Io {
                        path: relative,
                        source,
                    });
                }
            }
        }
    }

    fn relative(&self) -> &Path {
        &self.relative
    }

    fn file(&self) -> &File {
        &self.file
    }

    fn file_mut(&mut self) -> &mut File {
        &mut self.file
    }

    fn persist(mut self, target: &Path) -> Result<(), FileError> {
        self.directory
            .rename(&self.relative, &self.directory, target)
            .map_err(|source| FileError::Io {
                path: target.to_path_buf(),
                source,
            })?;
        self.persisted = true;
        Ok(())
    }
}

impl Drop for CapabilityTempFile {
    fn drop(&mut self) {
        if !self.persisted {
            let _ = self.directory.remove_file(&self.relative);
        }
    }
}

fn encode(note: &LoadedNote, content: &str) -> Vec<u8> {
    let mut normalized = content.replace("\r\n", "\n");
    if note.hash.is_some() {
        if note.had_final_newline {
            if !normalized.ends_with('\n') {
                normalized.push('\n');
            }
        } else {
            while normalized.ends_with('\n') {
                normalized.pop();
            }
        }
    }
    let encoded = match note.newline_style {
        NewlineStyle::Lf => normalized,
        NewlineStyle::CrLf => normalized.replace('\n', "\r\n"),
    };
    let mut bytes = Vec::with_capacity(encoded.len() + usize::from(note.has_bom) * UTF8_BOM.len());
    if note.has_bom {
        bytes.extend_from_slice(UTF8_BOM);
    }
    bytes.extend_from_slice(encoded.as_bytes());
    bytes
}

#[cfg(test)]
mod tests {
    use std::{fs, path::Path};

    use tempfile::tempdir;
    use uuid::Uuid;

    use crate::{
        catalog::RepoEntry,
        workspace::{FileError, Workspace},
    };

    use super::{reset_snapshot_read_count, save_note_before_commit, snapshot_read_count};

    #[test]
    fn final_conflict_check_does_not_read_target_content() {
        let sandbox = tempdir().unwrap();
        let root = fs::canonicalize(sandbox.path()).unwrap();
        let target = root.join("note.md");
        fs::write(&target, "loaded").unwrap();
        let workspace = Workspace::open(RepoEntry {
            id: Uuid::new_v4(),
            name: "notes".into(),
            path: root,
        })
        .unwrap();
        let note = workspace
            .load_note(&workspace.resolve_note(Path::new("note.md")).unwrap())
            .unwrap();
        reset_snapshot_read_count();

        save_note_before_commit(note, "editor".into(), false, || {}).unwrap();

        // One read verifies the loaded bytes; one builds the returned LoadedNote.
        assert_eq!(snapshot_read_count(), 2);
    }

    #[test]
    fn save_rechecks_for_external_edits_after_syncing_the_temporary_file() {
        let sandbox = tempdir().unwrap();
        let root = fs::canonicalize(sandbox.path()).unwrap();
        let target = root.join("note.md");
        fs::write(&target, "loaded").unwrap();
        let workspace = Workspace::open(RepoEntry {
            id: Uuid::new_v4(),
            name: "notes".into(),
            path: root,
        })
        .unwrap();
        let note = workspace
            .load_note(&workspace.resolve_note(Path::new("note.md")).unwrap())
            .unwrap();

        let error = save_note_before_commit(note, "editor".into(), false, || {
            fs::write(&target, "edited").unwrap();
        })
        .unwrap_err();

        assert!(matches!(error, FileError::ExternalModification { .. }));
        assert_eq!(fs::read_to_string(&target).unwrap(), "edited");
        assert_eq!(fs::read_dir(target.parent().unwrap()).unwrap().count(), 1);
    }

    #[test]
    fn save_rechecks_file_identity_after_syncing_the_temporary_file() {
        let sandbox = tempdir().unwrap();
        let root = fs::canonicalize(sandbox.path()).unwrap();
        let target = root.join("note.md");
        fs::write(&target, "same bytes").unwrap();
        let workspace = Workspace::open(RepoEntry {
            id: Uuid::new_v4(),
            name: "notes".into(),
            path: root,
        })
        .unwrap();
        let note = workspace
            .load_note(&workspace.resolve_note(Path::new("note.md")).unwrap())
            .unwrap();

        let error = save_note_before_commit(note, "editor".into(), false, || {
            fs::remove_file(&target).unwrap();
            fs::write(&target, "same bytes").unwrap();
        })
        .unwrap_err();

        assert!(matches!(error, FileError::ExternalModification { .. }));
        assert_eq!(fs::read_to_string(&target).unwrap(), "same bytes");
    }

    #[test]
    fn save_rechecks_for_external_deletion_after_syncing_the_temporary_file() {
        let sandbox = tempdir().unwrap();
        let root = fs::canonicalize(sandbox.path()).unwrap();
        let target = root.join("note.md");
        fs::write(&target, "loaded").unwrap();
        let workspace = Workspace::open(RepoEntry {
            id: Uuid::new_v4(),
            name: "notes".into(),
            path: root,
        })
        .unwrap();
        let note = workspace
            .load_note(&workspace.resolve_note(Path::new("note.md")).unwrap())
            .unwrap();

        let error = save_note_before_commit(note, "editor".into(), false, || {
            fs::remove_file(&target).unwrap();
        })
        .unwrap_err();

        assert!(matches!(error, FileError::ExternalDeletion { .. }));
        assert!(!target.exists());
    }

    #[test]
    fn first_save_rechecks_for_external_creation_after_syncing_the_temporary_file() {
        let sandbox = tempdir().unwrap();
        let root = fs::canonicalize(sandbox.path()).unwrap();
        let target = root.join("note.md");
        let workspace = Workspace::open(RepoEntry {
            id: Uuid::new_v4(),
            name: "notes".into(),
            path: root,
        })
        .unwrap();
        let note = workspace
            .load_note(&workspace.resolve_note(Path::new("note.md")).unwrap())
            .unwrap();

        let error = save_note_before_commit(note, "editor".into(), false, || {
            fs::write(&target, "external").unwrap();
        })
        .unwrap_err();

        assert!(matches!(error, FileError::ExternalModification { .. }));
        assert_eq!(fs::read_to_string(&target).unwrap(), "external");
    }
}
