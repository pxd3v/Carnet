use std::path::{Component, Path, PathBuf};

use thiserror::Error;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NotePath {
    root: PathBuf,
    relative: PathBuf,
}

impl NotePath {
    pub fn relative(&self) -> &Path {
        &self.relative
    }

    pub(crate) fn absolute(&self) -> PathBuf {
        self.root.join(&self.relative)
    }

    pub(crate) fn belongs_to(&self, root: &Path) -> bool {
        self.root == root
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }
}

pub(crate) fn revalidate_note(root: &Path, path: &NotePath) -> Result<(), PathError> {
    if !path.belongs_to(root) {
        return Err(PathError::Traversal {
            path: path.relative.clone(),
        });
    }
    validate_root(root)?;
    resolve_note(root, &path.relative).map(|_| ())
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
            Self::Io { .. } => "io",
        }
    }
}

pub(crate) fn resolve_note(root: &Path, path: &Path) -> Result<NotePath, PathError> {
    let resolved = resolve_target(root, path, false)?;
    Ok(NotePath {
        root: root.to_path_buf(),
        relative: resolved.relative,
    })
}

pub(crate) struct ResolvedPath {
    pub relative: PathBuf,
    pub absolute: PathBuf,
    pub metadata: Option<std::fs::Metadata>,
}

pub(crate) fn resolve_target(
    root: &Path,
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
            Component::Normal(name) if name == ".git" => {
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
    let absolute = root.join(&relative);
    let mut current = root.to_path_buf();
    for component in &relative {
        current.push(component);
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(PathError::Symlink { path: current });
            }
            Ok(metadata) if current == absolute && metadata.is_dir() && !allow_directory => {
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
    validate_root(root)?;

    let metadata = match std::fs::symlink_metadata(&absolute) {
        Ok(metadata) => Some(metadata),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => None,
        Err(source) => {
            return Err(PathError::Io {
                path: absolute.clone(),
                source,
            });
        }
    };
    Ok(ResolvedPath {
        relative,
        absolute,
        metadata,
    })
}

pub(crate) fn validate_root(root: &Path) -> Result<(), PathError> {
    let canonical_root = std::fs::canonicalize(root).map_err(|source| PathError::Io {
        path: root.to_path_buf(),
        source,
    })?;
    if canonical_root != root {
        return Err(PathError::Symlink {
            path: root.to_path_buf(),
        });
    }
    Ok(())
}
