use std::{
    path::{Component, Path, PathBuf},
    sync::Arc,
};

use cap_std::fs::Dir;
use thiserror::Error;

#[derive(Clone, Debug)]
pub struct NotePath {
    root: PathBuf,
    relative: PathBuf,
    directory: Arc<Dir>,
}

impl PartialEq for NotePath {
    fn eq(&self, other: &Self) -> bool {
        self.root == other.root && self.relative == other.relative
    }
}

impl Eq for NotePath {}

impl NotePath {
    pub fn relative(&self) -> &Path {
        &self.relative
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn directory(&self) -> &Arc<Dir> {
        &self.directory
    }
}

pub(crate) fn revalidate_note(path: &NotePath) -> Result<(), PathError> {
    resolve_target(&path.directory, &path.relative, false).map(|_| ())
}

#[derive(Debug, Error)]
pub enum PathError {
    #[error("path must be relative: {path}")]
    Absolute { path: PathBuf },
    #[error("path must not traverse outside the repository: {path}")]
    Traversal { path: PathBuf },
    #[error("path must not access Git metadata: {path}")]
    GitMetadata { path: PathBuf },
    #[error("note path is a directory: {path}")]
    DirectoryTarget { path: PathBuf },
    #[error("note path contains a symbolic link: {path}")]
    Symlink { path: PathBuf },
    #[error("opened repository root changed: {path}")]
    RootChanged { path: PathBuf },
    #[error("could not inspect path {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

impl PathError {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Absolute { .. } => "absolute",
            Self::Traversal { .. } => "traversal",
            Self::GitMetadata { .. } => "git",
            Self::DirectoryTarget { .. } => "directory",
            Self::Symlink { .. } => "symlink",
            Self::RootChanged { .. } => "root-changed",
            Self::Io { .. } => "io",
        }
    }
}

pub(crate) fn resolve_note(
    root: &Path,
    directory: Arc<Dir>,
    path: &Path,
) -> Result<NotePath, PathError> {
    let resolved = resolve_target(&directory, path, false)?;
    Ok(NotePath {
        root: root.to_path_buf(),
        relative: resolved.relative,
        directory,
    })
}

pub(crate) struct ResolvedPath {
    pub relative: PathBuf,
    pub metadata: Option<cap_std::fs::Metadata>,
}

pub(crate) fn resolve_target(
    directory: &Dir,
    path: &Path,
    allow_directory: bool,
) -> Result<ResolvedPath, PathError> {
    if path.is_absolute() {
        return Err(PathError::Absolute {
            path: path.to_path_buf(),
        });
    }
    let mut relative = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                return Err(PathError::Traversal {
                    path: path.to_path_buf(),
                });
            }
            Component::Normal(name) if name.as_encoded_bytes().eq_ignore_ascii_case(b".git") => {
                return Err(PathError::GitMetadata {
                    path: path.to_path_buf(),
                });
            }
            Component::Normal(name) => relative.push(name),
            Component::CurDir => {}
            Component::RootDir | Component::Prefix(_) => {
                return Err(PathError::Absolute {
                    path: path.to_path_buf(),
                });
            }
        }
    }
    let mut current = PathBuf::new();
    for component in &relative {
        current.push(component);
        match directory.symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(PathError::Symlink { path: current });
            }
            Ok(metadata) if current == relative && metadata.is_dir() && !allow_directory => {
                return Err(PathError::DirectoryTarget { path: relative });
            }
            Ok(_) => {}
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => break,
            Err(source) => {
                return Err(PathError::Io {
                    path: current,
                    source,
                });
            }
        }
    }
    if relative.as_os_str().is_empty() {
        return Err(PathError::DirectoryTarget { path: relative });
    }
    let metadata = match directory.symlink_metadata(&relative) {
        Ok(metadata) => Some(metadata),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => None,
        Err(source) => {
            return Err(PathError::Io {
                path: relative.clone(),
                source,
            });
        }
    };
    Ok(ResolvedPath { relative, metadata })
}
