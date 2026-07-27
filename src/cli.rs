use std::path::{Component, Path, PathBuf};

use clap::Parser;
use thiserror::Error;
use uuid::Uuid;

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
    Home {
        selected_repository: Option<Uuid>,
        pending_note: Option<PathBuf>,
    },
    Repository {
        repository: RepoEntry,
        note: Option<PathBuf>,
    },
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
    if let Some(note) = &cli.note_path {
        validate_note_path(note)?;
    }
    match (cli.repo.as_deref(), cli.note_path) {
        (None, None) => Ok(Launch::Home {
            selected_repository: catalog.default_repository_id(),
            pending_note: None,
        }),
        (None, Some(note)) => match catalog.resolve_repo(None) {
            Ok(repository) => Ok(Launch::Repository {
                repository,
                note: Some(note),
            }),
            Err(CatalogError::DefaultRepositoryNotSet) => Ok(Launch::Home {
                selected_repository: None,
                pending_note: Some(note),
            }),
            Err(error) => Err(error.into()),
        },
        (Some(name), note) => Ok(Launch::Repository {
            repository: catalog.resolve_repo(Some(name))?,
            note,
        }),
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
