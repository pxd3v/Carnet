mod files;
mod paths;
mod tree;

use std::path::{Path, PathBuf};

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
        Ok(Workspace {
            repo,
            root: canonical,
        })
    }

    pub fn resolve_note(&self, path: &Path) -> Result<NotePath, PathError> {
        paths::resolve_note(&self.root, path)
    }

    pub fn load_note(&self, path: &NotePath) -> Result<LoadedNote, FileError> {
        files::load_note(&self.root, path)
    }

    pub fn tree(&self) -> Result<Vec<TreeEntry>, FileError> {
        tree::build(&self.root)
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
}
