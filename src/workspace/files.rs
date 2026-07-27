use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use sha2::{Digest, Sha256};
use thiserror::Error;

use super::{NotePath, PathError, Workspace, paths};

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
    permissions: Option<fs::Permissions>,
}

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

pub(crate) fn load_note(root: &std::path::Path, path: &NotePath) -> Result<LoadedNote, FileError> {
    paths::revalidate_note(root, path)?;
    let absolute = path.absolute();
    let bytes = match fs::read(&absolute) {
        Ok(bytes) => bytes,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            return Ok(LoadedNote {
                path: path.clone(),
                text: String::new(),
                hash: None,
                has_bom: false,
                newline_style: NewlineStyle::Lf,
                had_final_newline: false,
                permissions: None,
            });
        }
        Err(source) => {
            return Err(FileError::Io {
                path: absolute,
                source,
            });
        }
    };
    if bytes.contains(&0) {
        return Err(FileError::Binary { path: absolute });
    }
    let hash = Sha256::digest(&bytes).into();
    let has_bom = bytes.starts_with(UTF8_BOM);
    let payload = if has_bom {
        &bytes[UTF8_BOM.len()..]
    } else {
        &bytes
    };
    let decoded = std::str::from_utf8(payload).map_err(|_| FileError::InvalidUtf8 {
        path: absolute.clone(),
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
    let permissions = fs::metadata(&absolute)
        .map_err(|source| FileError::Io {
            path: absolute,
            source,
        })?
        .permissions();

    Ok(LoadedNote {
        path: path.clone(),
        text,
        hash: Some(hash),
        has_bom,
        newline_style,
        had_final_newline,
        permissions: Some(permissions),
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
    let resolved = paths::resolve_target(workspace.root(), path, false)?;
    if resolved.metadata.is_some() {
        return Err(FileError::AlreadyExists {
            path: resolved.relative,
        });
    }
    let parent = resolved
        .absolute
        .parent()
        .expect("validated target is below repository root");
    fs::create_dir_all(parent).map_err(|source| FileError::Io {
        path: parent.to_path_buf(),
        source,
    })?;
    let resolved = paths::resolve_target(workspace.root(), path, false)?;
    let mut temporary =
        tempfile::NamedTempFile::new_in(parent).map_err(|source| FileError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    temporary.flush().map_err(|source| FileError::Io {
        path: temporary.path().to_path_buf(),
        source,
    })?;
    temporary
        .as_file()
        .sync_all()
        .map_err(|source| FileError::Io {
            path: temporary.path().to_path_buf(),
            source,
        })?;
    temporary
        .persist_noclobber(&resolved.absolute)
        .map_err(|error| FileError::Io {
            path: resolved.absolute,
            source: error.error,
        })?;
    Ok(FileOutcome::CreatedFile(paths::resolve_note(
        workspace.root(),
        path,
    )?))
}

fn create_folder(workspace: &Workspace, path: &Path) -> Result<FileOutcome, FileError> {
    let resolved = paths::resolve_target(workspace.root(), path, true)?;
    if resolved.metadata.is_some() {
        return Err(FileError::AlreadyExists {
            path: resolved.relative,
        });
    }
    fs::create_dir_all(&resolved.absolute).map_err(|source| FileError::Io {
        path: resolved.absolute.clone(),
        source,
    })?;
    paths::resolve_target(workspace.root(), path, true)?;
    Ok(FileOutcome::CreatedFolder(resolved.relative))
}

fn relocate(
    workspace: &Workspace,
    from: &Path,
    to: &Path,
    is_move: bool,
) -> Result<FileOutcome, FileError> {
    let source = paths::resolve_target(workspace.root(), from, true)?;
    if source.metadata.is_none() {
        return Err(FileError::Missing {
            path: source.relative,
        });
    }
    let destination = paths::resolve_target(workspace.root(), to, true)?;
    if destination.metadata.is_some() {
        return Err(FileError::AlreadyExists {
            path: destination.relative,
        });
    }
    let parent = destination
        .absolute
        .parent()
        .expect("validated target is below repository root");
    fs::create_dir_all(parent).map_err(|source| FileError::Io {
        path: parent.to_path_buf(),
        source,
    })?;
    let source = paths::resolve_target(workspace.root(), from, true)?;
    let destination = paths::resolve_target(workspace.root(), to, true)?;
    fs::rename(&source.absolute, &destination.absolute).map_err(|source| FileError::Io {
        path: destination.absolute,
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
    let resolved = paths::resolve_target(workspace.root(), path, true)?;
    let metadata = resolved.metadata.ok_or_else(|| FileError::Missing {
        path: resolved.relative.clone(),
    })?;
    if metadata.is_dir() {
        fs::remove_dir_all(&resolved.absolute)
    } else {
        fs::remove_file(&resolved.absolute)
    }
    .map_err(|source| FileError::Io {
        path: resolved.absolute,
        source,
    })?;
    Ok(FileOutcome::Deleted(resolved.relative))
}

fn save_note(note: LoadedNote, content: String, overwrite: bool) -> Result<FileOutcome, FileError> {
    paths::revalidate_note(note.path.root(), &note.path)?;
    let target = note.path.absolute();
    if !overwrite {
        verify_unchanged(&note, &target)?;
    }
    let parent = target.parent().expect("note path has a repository root");
    fs::create_dir_all(parent).map_err(|source| FileError::Io {
        path: parent.to_path_buf(),
        source,
    })?;
    paths::revalidate_note(note.path.root(), &note.path)?;

    let bytes = encode(&note, &content);
    let mut temporary =
        tempfile::NamedTempFile::new_in(parent).map_err(|source| FileError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    temporary
        .write_all(&bytes)
        .map_err(|source| FileError::Io {
            path: temporary.path().to_path_buf(),
            source,
        })?;
    if let Some(permissions) = note.permissions.clone() {
        temporary
            .as_file()
            .set_permissions(permissions)
            .map_err(|source| FileError::Io {
                path: temporary.path().to_path_buf(),
                source,
            })?;
    }
    temporary.flush().map_err(|source| FileError::Io {
        path: temporary.path().to_path_buf(),
        source,
    })?;
    temporary
        .as_file()
        .sync_all()
        .map_err(|source| FileError::Io {
            path: temporary.path().to_path_buf(),
            source,
        })?;
    temporary.persist(&target).map_err(|error| FileError::Io {
        path: target.clone(),
        source: error.error,
    })?;
    Ok(FileOutcome::Saved(load_note(note.path.root(), &note.path)?))
}

fn verify_unchanged(note: &LoadedNote, target: &Path) -> Result<(), FileError> {
    match (&note.hash, fs::read(target)) {
        (Some(_), Err(source)) if source.kind() == std::io::ErrorKind::NotFound => {
            Err(FileError::ExternalDeletion {
                path: note.path.relative().to_path_buf(),
            })
        }
        (None, Err(source)) if source.kind() == std::io::ErrorKind::NotFound => Ok(()),
        (Some(expected), Ok(bytes)) if <[u8; 32]>::from(Sha256::digest(&bytes)) == *expected => {
            Ok(())
        }
        (Some(_), Ok(_)) | (None, Ok(_)) => Err(FileError::ExternalModification {
            path: note.path.relative().to_path_buf(),
        }),
        (_, Err(source)) => Err(FileError::Io {
            path: target.to_path_buf(),
            source,
        }),
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
