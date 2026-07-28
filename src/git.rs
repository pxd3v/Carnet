use std::{
    collections::BTreeSet,
    ffi::OsStr,
    os::fd::AsRawFd,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use cap_std::{ambient_authority, fs::Dir};
use std::os::unix::process::CommandExt;
use thiserror::Error;

use crate::workspace::{DirectoryIdentity, FileError, FileOperation, FileOutcome, Workspace};

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PushOutcome {
    Pushed,
    UpToDate,
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
pub enum MutationCommitError {
    #[error(transparent)]
    File(#[from] FileError),
    #[error("file operation belongs to a different workspace")]
    WorkspaceMismatch,
    #[error("Git repository belongs to a different workspace")]
    RepositoryMismatch,
    #[error("background mutation worker panicked: {message}")]
    Runtime { message: String },
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
    #[error("background {operation} worker panicked")]
    WorkerPanicked { operation: &'static str },
    #[error("opened repository root changed: {path}")]
    RootChanged { path: PathBuf },
    #[error("git {operation} was cancelled during shutdown")]
    Cancelled { operation: &'static str },
}

#[derive(Clone, Debug)]
pub struct GitRepo {
    root: PathBuf,
    directory: Arc<Dir>,
    identity: DirectoryIdentity,
    cancellation: GitCancellation,
}

#[derive(Clone, Debug)]
pub(crate) struct GitCancellation {
    state: Arc<GitCancellationState>,
}

#[derive(Debug, Default)]
struct GitCancellationState {
    cancelled: AtomicBool,
    process_groups: Mutex<BTreeSet<i32>>,
}

impl Default for GitCancellation {
    fn default() -> Self {
        Self {
            state: Arc::new(GitCancellationState::default()),
        }
    }
}

impl GitCancellation {
    pub(crate) fn cancel(&self) {
        self.state.cancelled.store(true, Ordering::SeqCst);
        let groups = self
            .state
            .process_groups
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        for process_group in groups.iter().copied() {
            // SAFETY: negative pid targets the child-owned process group.
            unsafe {
                libc::kill(-process_group, libc::SIGKILL);
            }
        }
    }

    fn register(&self, process_group: i32) {
        let mut groups = self
            .state
            .process_groups
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        groups.insert(process_group);
        if self.state.cancelled.load(Ordering::SeqCst) {
            // SAFETY: negative pid targets the just-spawned child process group.
            unsafe {
                libc::kill(-process_group, libc::SIGKILL);
            }
        }
    }

    fn unregister(&self, process_group: i32) {
        self.state
            .process_groups
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&process_group);
    }

    fn is_cancelled(&self) -> bool {
        self.state.cancelled.load(Ordering::SeqCst)
    }
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
        let input_directory =
            Dir::open_ambient_dir(&canonical, ambient_authority()).map_err(|source| {
                GitError::Io {
                    operation: "open",
                    source,
                }
            })?;
        let cancellation = GitCancellation::default();
        let output = run_from_directory(
            &input_directory,
            &cancellation,
            "rev-parse",
            [OsStr::new("rev-parse"), OsStr::new("--show-toplevel")],
        )?;
        if !output.status.success() {
            return Err(GitError::NotWorkTree { path: canonical });
        }
        let root = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
        let root = std::fs::canonicalize(&root).map_err(|source| GitError::Io {
            operation: "open",
            source,
        })?;
        let directory =
            Dir::open_ambient_dir(&root, ambient_authority()).map_err(|source| GitError::Io {
                operation: "open",
                source,
            })?;
        let identity = DirectoryIdentity::from_dir(&directory).map_err(|source| GitError::Io {
            operation: "open",
            source,
        })?;
        Ok(Self {
            root,
            directory: Arc::new(directory),
            identity,
            cancellation,
        })
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

    pub fn push(&self) -> Result<PushOutcome, GitError> {
        let output = self.run_checked("push", [OsStr::new("push"), OsStr::new("--porcelain")])?;
        let report = String::from_utf8_lossy(&output.stdout);
        Ok(if report.lines().any(|line| line.starts_with('=')) {
            PushOutcome::UpToDate
        } else {
            PushOutcome::Pushed
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn identity(&self) -> DirectoryIdentity {
        self.identity
    }

    pub(crate) fn cancellation(&self) -> GitCancellation {
        self.cancellation.clone()
    }

    fn run<I, S>(&self, operation: &'static str, args: I) -> Result<Output, GitError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        match DirectoryIdentity::from_ambient(&self.root) {
            Ok(identity) if identity == self.identity => {}
            Ok(_) => {
                return Err(GitError::RootChanged {
                    path: self.root.clone(),
                });
            }
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                return Err(GitError::RootChanged {
                    path: self.root.clone(),
                });
            }
            Err(source) => return Err(GitError::Io { operation, source }),
        }
        run_from_directory(&self.directory, &self.cancellation, operation, args)
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
    workspace: &Workspace,
    repo: &GitRepo,
    operation: FileOperation,
    intent: CommitIntent,
) -> Result<MutationCommitOutcome, MutationCommitError> {
    if operation.workspace_root() != workspace.root()
        || operation.workspace_identity() != Some(workspace.identity())
    {
        return Err(MutationCommitError::WorkspaceMismatch);
    }
    if repo.identity() != workspace.identity() {
        return Err(MutationCommitError::RepositoryMismatch);
    }
    let file = Workspace::apply(operation)?;
    Ok(match repo.commit_all(intent) {
        Ok(commit) => MutationCommitOutcome::Applied { file, commit },
        Err(error) => MutationCommitOutcome::SavedCommitFailed { file, error },
    })
}

fn run_from_directory<I, S>(
    directory: &Dir,
    cancellation: &GitCancellation,
    operation: &'static str,
    args: I,
) -> Result<Output, GitError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    if cancellation.is_cancelled() {
        return Err(GitError::Cancelled { operation });
    }
    let directory_fd = directory.as_raw_fd();
    let mut command = Command::new("git");
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // SAFETY: setpgid and fchdir are async-signal-safe, and `directory` keeps
    // its fd alive until spawning finishes. The child owns the new group.
    unsafe {
        command.pre_exec(move || {
            if libc::setpgid(0, 0) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::fchdir(directory_fd) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let child = command
        .spawn()
        .map_err(|source| GitError::Io { operation, source })?;
    let process_group = i32::try_from(child.id()).map_err(|_| GitError::Io {
        operation,
        source: std::io::Error::other("child pid did not fit in a process-group id"),
    })?;
    cancellation.register(process_group);
    let output = child
        .wait_with_output()
        .map_err(|source| GitError::Io { operation, source });
    cancellation.unregister(process_group);
    let output = output?;
    if cancellation.is_cancelled() {
        Err(GitError::Cancelled { operation })
    } else {
        Ok(output)
    }
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
