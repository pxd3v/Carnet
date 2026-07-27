use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use thiserror::Error;

use crate::workspace::{FileError, FileOperation, FileOutcome, Workspace};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CommitIntent {
    Create(PathBuf),
    Update(PathBuf),
    Move { from: PathBuf, to: PathBuf },
    Delete(PathBuf),
}

impl CommitIntent {
    fn message(&self) -> String {
        match self {
            Self::Create(path) => format!("carnet: create {}", path.display()),
            Self::Update(path) => format!("carnet: update {}", path.display()),
            Self::Move { from, to } => {
                format!("carnet: move {} to {}", from.display(), to.display())
            }
            Self::Delete(path) => format!("carnet: delete {}", path.display()),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CommitOutcome {
    Committed { revision: String },
    NoChanges,
}

#[derive(Debug)]
pub enum MutationCommitOutcome {
    Applied {
        file: FileOutcome,
        commit: CommitOutcome,
    },
    SavedCommitFailed {
        file: FileOutcome,
        error: GitError,
    },
}

#[derive(Debug, Error)]
pub enum GitError {
    #[error("could not run git {operation}: {source}")]
    Io {
        operation: &'static str,
        #[source]
        source: std::io::Error,
    },
    #[error("path is not a Git work tree: {path}")]
    NotWorkTree { path: PathBuf },
    #[error("git {operation} failed: {stderr}")]
    CommandFailed {
        operation: &'static str,
        status: Option<i32>,
        stderr: String,
    },
}

#[derive(Clone, Debug)]
pub struct GitRepo {
    root: PathBuf,
}

impl GitRepo {
    pub fn initialize(path: &Path) -> Result<GitRepo, GitError> {
        let output = Command::new("git")
            .arg("init")
            .arg(path)
            .output()
            .map_err(|source| GitError::Io {
                operation: "init",
                source,
            })?;
        check_status("init", output)?;
        Self::open(path)
    }

    pub fn open(path: &Path) -> Result<GitRepo, GitError> {
        let canonical = std::fs::canonicalize(path).map_err(|source| GitError::Io {
            operation: "open",
            source,
        })?;
        let output = Command::new("git")
            .args(["rev-parse", "--show-toplevel"])
            .current_dir(&canonical)
            .output()
            .map_err(|source| GitError::Io {
                operation: "rev-parse",
                source,
            })?;
        if !output.status.success() {
            return Err(GitError::NotWorkTree { path: canonical });
        }
        let root = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
        let root = std::fs::canonicalize(&root).map_err(|source| GitError::Io {
            operation: "open",
            source,
        })?;
        Ok(Self { root })
    }

    pub fn commit_all(&self, intent: CommitIntent) -> Result<CommitOutcome, GitError> {
        self.run_checked("add", [OsStr::new("add"), OsStr::new("-A")])?;

        let diff = self.run(
            "diff",
            [
                OsStr::new("diff"),
                OsStr::new("--cached"),
                OsStr::new("--quiet"),
            ],
        )?;
        match diff.status.code() {
            Some(0) => return Ok(CommitOutcome::NoChanges),
            Some(1) => {}
            _ => return Err(command_failed("diff", diff)),
        }

        let message = intent.message();
        self.run_checked(
            "commit",
            [OsStr::new("commit"), OsStr::new("-m"), message.as_ref()],
        )?;
        let output =
            self.run_checked("rev-parse", [OsStr::new("rev-parse"), OsStr::new("HEAD")])?;
        Ok(CommitOutcome::Committed {
            revision: String::from_utf8_lossy(&output.stdout).trim().to_owned(),
        })
    }

    fn run<I, S>(&self, operation: &'static str, args: I) -> Result<Output, GitError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        Command::new("git")
            .args(args)
            .current_dir(&self.root)
            .output()
            .map_err(|source| GitError::Io { operation, source })
    }

    fn run_checked<I, S>(&self, operation: &'static str, args: I) -> Result<Output, GitError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        check_status(operation, self.run(operation, args)?)
    }
}

pub fn apply_and_commit(
    _workspace: &Workspace,
    repo: &GitRepo,
    operation: FileOperation,
    intent: CommitIntent,
) -> Result<MutationCommitOutcome, FileError> {
    let file = Workspace::apply(operation)?;
    Ok(match repo.commit_all(intent) {
        Ok(commit) => MutationCommitOutcome::Applied { file, commit },
        Err(error) => MutationCommitOutcome::SavedCommitFailed { file, error },
    })
}

fn check_status(operation: &'static str, output: Output) -> Result<Output, GitError> {
    if output.status.success() {
        Ok(output)
    } else {
        Err(command_failed(operation, output))
    }
}

fn command_failed(operation: &'static str, output: Output) -> GitError {
    GitError::CommandFailed {
        operation,
        status: output.status.code(),
        stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
    }
}
