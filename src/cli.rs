use std::path::{Component, Path, PathBuf};

use clap::Parser;
use thiserror::Error;

use crate::catalog::{Catalog, CatalogError, RepoEntry};

#[derive(Debug, Parser)]
#[command(
    name = "carnet",
    version,
    about = "A terminal note editor with Git-backed repositories",
    long_about = "Open a registered repository or a note within one. Carnet keeps note files in ordinary Git repositories."
)]
pub struct Cli {
    /// Select a registered repository by name.
    #[arg(short, long, value_name = "NAME")]
    pub repo: Option<String>,

    /// Note to open or prepare, relative to the selected repository.
    #[arg(value_name = "NOTE_PATH")]
    pub note_path: Option<PathBuf>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Launch {
    RepositoryHome { default_repo: RepoEntry },
    Note { repo: RepoEntry, note_path: PathBuf },
}

#[derive(Debug, Error)]
pub enum CliError {
    #[error(transparent)]
    Catalog(#[from] CatalogError),
    #[error("note path must be relative: {path}")]
    AbsoluteNotePath { path: PathBuf },
    #[error("note path must not traverse outside its repository: {path}")]
    TraversalNotePath { path: PathBuf },
}

pub fn route(cli: Cli, catalog: &Catalog) -> Result<Launch, CliError> {
    let repo = catalog.resolve_repo(cli.repo.as_deref())?;
    match cli.note_path {
        Some(note_path) => {
            validate_note_path(&note_path)?;
            Ok(Launch::Note { repo, note_path })
        }
        None => Ok(Launch::RepositoryHome { default_repo: repo }),
    }
}

fn validate_note_path(path: &Path) -> Result<(), CliError> {
    if path.is_absolute() {
        return Err(CliError::AbsoluteNotePath {
            path: path.to_path_buf(),
        });
    }
    if path
        .components()
        .any(|component| component == Component::ParentDir)
    {
        return Err(CliError::TraversalNotePath {
            path: path.to_path_buf(),
        });
    }
    Ok(())
}
