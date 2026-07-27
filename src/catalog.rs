use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;
use thiserror::Error;
use uuid::Uuid;

const CATALOG_VERSION: u32 = 1;
const CATALOG_FILENAME: &str = "catalog.toml";

#[derive(Debug, Error)]
pub enum CatalogError {
    #[error("the operating system did not provide a configuration directory")]
    ConfigDirectoryUnavailable,
    #[error("could not read or write catalog at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("could not parse catalog at {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
    #[error("could not serialize catalog: {0}")]
    Serialize(#[from] toml::ser::Error),
    #[error("catalog version {found} is not supported")]
    UnsupportedVersion { found: u32 },
    #[error("repository name must not be empty")]
    EmptyName,
    #[error("a repository named {name:?} is already registered")]
    DuplicateName { name: String },
    #[error("repository named {name:?} is not registered")]
    RepositoryNotFound { name: String },
    #[error("the default repository is not registered")]
    DefaultRepositoryNotSet,
    #[error("repository path does not exist: {path}")]
    RepositoryPathMissing { path: PathBuf },
    #[error("repository path is not a directory: {path}")]
    RepositoryPathNotDirectory { path: PathBuf },
    #[error("repository path is not canonical: {path}")]
    RepositoryPathNotCanonical { path: PathBuf },
    #[error("catalog contains duplicate repository ID {id}")]
    DuplicateId { id: Uuid },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RepoEntry {
    pub id: Uuid,
    pub name: String,
    pub path: PathBuf,
}

#[derive(Clone, Debug)]
pub struct Catalog {
    path: PathBuf,
    version: u32,
    default_repo_id: Option<Uuid>,
    repos: Vec<RepoEntry>,
}

#[derive(Deserialize, Serialize)]
struct CatalogFile {
    version: u32,
    default_repo_id: Option<Uuid>,
    repos: Vec<RepoEntry>,
}

impl Catalog {
    pub fn load() -> Result<Catalog, CatalogError> {
        Self::load_at(Self::config_path()?)
    }

    pub fn save(&self) -> Result<(), CatalogError> {
        let contents = toml::to_string_pretty(&CatalogFile {
            version: self.version,
            default_repo_id: self.default_repo_id,
            repos: self.repos.clone(),
        })?;
        let directory = parent_directory(&self.path);
        fs::create_dir_all(directory).map_err(|source| CatalogError::Io {
            path: directory.to_path_buf(),
            source,
        })?;

        let mut temporary =
            NamedTempFile::new_in(directory).map_err(|source| CatalogError::Io {
                path: directory.to_path_buf(),
                source,
            })?;
        temporary
            .write_all(contents.as_bytes())
            .map_err(|source| CatalogError::Io {
                path: temporary.path().to_path_buf(),
                source,
            })?;
        temporary.flush().map_err(|source| CatalogError::Io {
            path: temporary.path().to_path_buf(),
            source,
        })?;
        temporary
            .as_file()
            .sync_all()
            .map_err(|source| CatalogError::Io {
                path: temporary.path().to_path_buf(),
                source,
            })?;
        temporary
            .persist(&self.path)
            .map_err(|error| CatalogError::Io {
                path: self.path.clone(),
                source: error.error,
            })?;

        Ok(())
    }

    pub fn resolve_repo(&self, name: Option<&str>) -> Result<RepoEntry, CatalogError> {
        let repo = match name {
            Some(name) => self
                .repos
                .iter()
                .find(|repo| repo.name == name)
                .ok_or_else(|| CatalogError::RepositoryNotFound {
                    name: name.to_owned(),
                })?,
            None => {
                let id = self
                    .default_repo_id
                    .ok_or(CatalogError::DefaultRepositoryNotSet)?;
                self.repos
                    .iter()
                    .find(|repo| repo.id == id)
                    .ok_or(CatalogError::DefaultRepositoryNotSet)?
            }
        };

        validate_canonical_repository_path(&repo.path)?;

        Ok(repo.clone())
    }

    pub fn repositories(&self) -> &[RepoEntry] {
        &self.repos
    }

    pub fn default_repository_id(&self) -> Option<Uuid> {
        self.default_repo_id
    }

    pub fn create() -> Result<Catalog, CatalogError> {
        Ok(Self::create_at(Self::config_path()?))
    }

    pub fn create_at(path: impl Into<PathBuf>) -> Catalog {
        Catalog {
            path: path.into(),
            version: CATALOG_VERSION,
            default_repo_id: None,
            repos: Vec::new(),
        }
    }

    pub fn load_at(path: impl Into<PathBuf>) -> Result<Catalog, CatalogError> {
        let path = path.into();
        let contents = match fs::read_to_string(&path) {
            Ok(contents) => contents,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::create_at(path));
            }
            Err(source) => return Err(CatalogError::Io { path, source }),
        };
        let file: CatalogFile =
            toml::from_str(&contents).map_err(|source| CatalogError::Parse {
                path: path.clone(),
                source,
            })?;
        if file.version != CATALOG_VERSION {
            return Err(CatalogError::UnsupportedVersion {
                found: file.version,
            });
        }
        validate_entries(&file.repos)?;
        for repo in &file.repos {
            validate_canonical_repository_path(&repo.path)?;
        }
        if let Some(default_repo_id) = file.default_repo_id
            && !file.repos.iter().any(|repo| repo.id == default_repo_id)
        {
            return Err(CatalogError::DefaultRepositoryNotSet);
        }

        Ok(Catalog {
            path,
            version: file.version,
            default_repo_id: file.default_repo_id,
            repos: file.repos,
        })
    }

    pub fn register(
        &mut self,
        name: impl Into<String>,
        path: impl AsRef<Path>,
    ) -> Result<RepoEntry, CatalogError> {
        let name = valid_name(name.into())?;
        if self.repos.iter().any(|repo| repo.name == name) {
            return Err(CatalogError::DuplicateName { name });
        }
        let canonical_path = canonical_repository_path(path.as_ref())?;
        let repo = RepoEntry {
            id: Uuid::new_v4(),
            name,
            path: canonical_path,
        };
        if self.default_repo_id.is_none() {
            self.default_repo_id = Some(repo.id);
        }
        self.repos.push(repo.clone());
        Ok(repo)
    }

    pub fn rename_registration(
        &mut self,
        current: &str,
        new: impl Into<String>,
    ) -> Result<(), CatalogError> {
        let new = valid_name(new.into())?;
        let repo_index = self
            .repos
            .iter()
            .position(|repo| repo.name == current)
            .ok_or_else(|| CatalogError::RepositoryNotFound {
                name: current.to_owned(),
            })?;
        if new != current && self.repos.iter().any(|repo| repo.name == new) {
            return Err(CatalogError::DuplicateName { name: new });
        }
        self.repos[repo_index].name = new;
        Ok(())
    }

    pub fn set_default(&mut self, name: &str) -> Result<(), CatalogError> {
        let repo = self
            .repos
            .iter()
            .find(|repo| repo.name == name)
            .ok_or_else(|| CatalogError::RepositoryNotFound {
                name: name.to_owned(),
            })?;
        self.default_repo_id = Some(repo.id);
        Ok(())
    }

    pub fn unregister(&mut self, name: &str) -> Result<RepoEntry, CatalogError> {
        let repo_index = self
            .repos
            .iter()
            .position(|repo| repo.name == name)
            .ok_or_else(|| CatalogError::RepositoryNotFound {
                name: name.to_owned(),
            })?;
        let removed = self.repos.remove(repo_index);
        if self.default_repo_id == Some(removed.id) {
            self.default_repo_id = None;
        }
        Ok(removed)
    }

    pub fn config_path() -> Result<PathBuf, CatalogError> {
        let directories =
            ProjectDirs::from("", "", "carnet").ok_or(CatalogError::ConfigDirectoryUnavailable)?;
        Ok(directories.config_dir().join(CATALOG_FILENAME))
    }
}

fn parent_directory(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

fn canonical_repository_path(path: &Path) -> Result<PathBuf, CatalogError> {
    let canonical_path = fs::canonicalize(path).map_err(|source| match source.kind() {
        std::io::ErrorKind::NotFound => CatalogError::RepositoryPathMissing {
            path: path.to_path_buf(),
        },
        _ => CatalogError::Io {
            path: path.to_path_buf(),
            source,
        },
    })?;
    if !canonical_path.is_dir() {
        return Err(CatalogError::RepositoryPathNotDirectory {
            path: canonical_path,
        });
    }
    Ok(canonical_path)
}

fn validate_canonical_repository_path(path: &Path) -> Result<(), CatalogError> {
    let canonical_path = canonical_repository_path(path)?;
    if canonical_path != path {
        return Err(CatalogError::RepositoryPathNotCanonical {
            path: path.to_path_buf(),
        });
    }
    Ok(())
}

fn valid_name(name: String) -> Result<String, CatalogError> {
    if name.trim().is_empty() {
        return Err(CatalogError::EmptyName);
    }
    Ok(name)
}

fn validate_entries(entries: &[RepoEntry]) -> Result<(), CatalogError> {
    for (index, repo) in entries.iter().enumerate() {
        valid_name(repo.name.clone())?;
        if entries[..index].iter().any(|other| other.name == repo.name) {
            return Err(CatalogError::DuplicateName {
                name: repo.name.clone(),
            });
        }
        if entries[..index].iter().any(|other| other.id == repo.id) {
            return Err(CatalogError::DuplicateId { id: repo.id });
        }
    }
    Ok(())
}
