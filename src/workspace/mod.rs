mod files;
mod paths;
mod tree;

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

#[cfg(unix)]
use cap_std::fs::MetadataExt;
use cap_std::{ambient_authority, fs::Dir};
use thiserror::Error;

use crate::catalog::RepoEntry;

pub use files::{FileError, FileOperation, FileOutcome, LoadedNote, NewlineStyle};
pub use paths::{NotePath, PathError};
pub use tree::{TreeEntry, TreeEntryKind};

#[derive(Debug, Error)]
pub enum WorkspaceError {
    #[error("repository path is not canonical: {path}")]
    NonCanonicalRoot { path: PathBuf },
    #[error("could not inspect repository at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

#[derive(Clone, Debug)]
pub struct Workspace {
    repo: RepoEntry,
    root: PathBuf,
    directory: Arc<Dir>,
    identity: DirectoryIdentity,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct DirectoryIdentity {
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}

impl DirectoryIdentity {
    pub(crate) fn from_dir(directory: &Dir) -> std::io::Result<Self> {
        let metadata = directory.dir_metadata()?;
        Ok(Self {
            #[cfg(unix)]
            device: metadata.dev(),
            #[cfg(unix)]
            inode: metadata.ino(),
        })
    }

    pub(crate) fn from_ambient(path: &Path) -> std::io::Result<Self> {
        let metadata = std::fs::metadata(path)?;
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;
        Ok(Self {
            #[cfg(unix)]
            device: metadata.dev(),
            #[cfg(unix)]
            inode: metadata.ino(),
        })
    }
}

impl Workspace {
    pub fn open(repo: RepoEntry) -> Result<Workspace, WorkspaceError> {
        let canonical = std::fs::canonicalize(&repo.path).map_err(|source| WorkspaceError::Io {
            path: repo.path.clone(),
            source,
        })?;
        if canonical != repo.path || !canonical.is_dir() {
            return Err(WorkspaceError::NonCanonicalRoot {
                path: repo.path.clone(),
            });
        }
        let directory =
            Dir::open_ambient_dir(&canonical, ambient_authority()).map_err(|source| {
                WorkspaceError::Io {
                    path: canonical.clone(),
                    source,
                }
            })?;
        let identity =
            DirectoryIdentity::from_dir(&directory).map_err(|source| WorkspaceError::Io {
                path: canonical.clone(),
                source,
            })?;
        Ok(Workspace {
            repo,
            root: canonical,
            directory: Arc::new(directory),
            identity,
        })
    }

    pub fn resolve_note(&self, path: &Path) -> Result<NotePath, PathError> {
        paths::resolve_note(&self.root, Arc::clone(&self.directory), path)
    }

    pub fn load_note(&self, path: &NotePath) -> Result<LoadedNote, FileError> {
        files::load_note(path)
    }

    pub fn tree(&self) -> Result<Vec<TreeEntry>, FileError> {
        self.ensure_registered_root()?;
        tree::build(&self.directory, &self.root)
    }

    pub fn apply(operation: FileOperation) -> Result<FileOutcome, FileError> {
        files::apply(operation)
    }

    pub fn repo(&self) -> &RepoEntry {
        &self.repo
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn directory(&self) -> &Arc<Dir> {
        &self.directory
    }

    pub(crate) fn identity(&self) -> DirectoryIdentity {
        self.identity
    }

    fn ensure_registered_root(&self) -> Result<(), PathError> {
        match DirectoryIdentity::from_ambient(&self.root) {
            Ok(identity) if identity == self.identity => Ok(()),
            Ok(_) => Err(PathError::RootChanged {
                path: self.root.clone(),
            }),
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                Err(PathError::RootChanged {
                    path: self.root.clone(),
                })
            }
            Err(source) => Err(PathError::Io {
                path: self.root.clone(),
                source,
            }),
        }
    }
}
