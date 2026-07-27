use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use crate::{
    catalog::RepoEntry,
    git::{
        CommitIntent, GitError, GitRepo, MutationCommitError, MutationCommitOutcome,
        apply_and_commit,
    },
    workspace::{FileError, FileOperation, PathError, Workspace, WorkspaceError},
};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug)]
pub enum AppEffect {
    OpenWorkspace {
        request_id: super::RequestId,
        repository: RepoEntry,
        note: Option<PathBuf>,
    },
    SetDefaultRepository {
        repository_id: Uuid,
    },
    CreateRepository {
        name: String,
        path: PathBuf,
    },
    RegisterRepository {
        name: String,
        path: PathBuf,
    },
    RenameRepository {
        repository_id: Uuid,
        name: String,
    },
    UnregisterRepository {
        repository_id: Uuid,
    },
    ReadClipboard,
    WriteClipboard {
        text: String,
    },
    ApplyAndCommit {
        mutation_id: super::MutationId,
        repository_id: Uuid,
        repository_root: PathBuf,
        workspace: Workspace,
        git: GitRepo,
        operation: Box<FileOperation>,
        intent: CommitIntent,
    },
    LoadNote {
        request_id: super::RequestId,
        repository_id: Uuid,
        workspace: Workspace,
        path: PathBuf,
    },
    RetryCommit {
        mutation_id: super::MutationId,
        repository_id: Uuid,
        repository_root: PathBuf,
        git: GitRepo,
        intent: CommitIntent,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeOperation {
    OpenWorkspace,
    LoadNote,
    Mutation,
    RefreshTree,
}

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error(transparent)]
    Workspace(#[from] WorkspaceError),
    #[error(transparent)]
    Git(#[from] GitError),
    #[error(transparent)]
    Path(#[from] PathError),
    #[error(transparent)]
    File(#[from] FileError),
    #[error("background {operation} worker panicked")]
    WorkerPanicked { operation: &'static str },
}

#[derive(Debug, Error)]
#[error("this effect belongs to an outer runtime boundary: {effect:?}")]
pub struct EffectExecutionError {
    effect: Box<AppEffect>,
}

impl EffectExecutionError {
    pub fn into_effect(self) -> AppEffect {
        *self.effect
    }
}

#[derive(Clone, Default)]
pub struct EffectExecutor {
    repository_locks: Arc<Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>>,
}

struct MutationOrigin {
    mutation_id: super::MutationId,
    repository_id: Uuid,
    repository_root: PathBuf,
}

impl EffectExecutor {
    pub fn execute(&self, effect: AppEffect) -> Result<super::AppEvent, EffectExecutionError> {
        match effect {
            AppEffect::OpenWorkspace {
                request_id,
                repository,
                note,
            } => Ok(self.open_workspace(request_id, repository, note)),
            AppEffect::LoadNote {
                request_id,
                repository_id,
                workspace,
                path,
            } => Ok(self.load_note(request_id, repository_id, workspace, path)),
            AppEffect::ApplyAndCommit {
                mutation_id,
                repository_id,
                repository_root,
                workspace,
                git,
                operation,
                intent,
            } => Ok(self.apply_and_commit(
                MutationOrigin {
                    mutation_id,
                    repository_id,
                    repository_root,
                },
                workspace,
                git,
                *operation,
                intent,
            )),
            AppEffect::RetryCommit {
                mutation_id,
                repository_id,
                repository_root,
                git,
                intent,
            } => Ok(self.retry_commit(
                MutationOrigin {
                    mutation_id,
                    repository_id,
                    repository_root,
                },
                git,
                intent,
            )),
            effect @ (AppEffect::SetDefaultRepository { .. }
            | AppEffect::CreateRepository { .. }
            | AppEffect::RegisterRepository { .. }
            | AppEffect::RenameRepository { .. }
            | AppEffect::UnregisterRepository { .. }
            | AppEffect::ReadClipboard
            | AppEffect::WriteClipboard { .. }) => Err(EffectExecutionError {
                effect: Box::new(effect),
            }),
        }
    }

    fn open_workspace(
        &self,
        request_id: super::RequestId,
        repository: RepoEntry,
        note_path: Option<PathBuf>,
    ) -> super::AppEvent {
        let repository_id = repository.id;
        let root = repository.path.clone();
        let result = self.run_serialized(&root, || {
            let workspace = Workspace::open(repository)?;
            let git = GitRepo::open(workspace.root())?;
            let tree = workspace.tree()?;
            let note = note_path
                .map(|path| {
                    let path = workspace.resolve_note(&path)?;
                    workspace.load_note(&path).map_err(RuntimeError::from)
                })
                .transpose()?;
            Ok::<_, RuntimeError>((workspace, git, tree, note))
        });
        match result {
            Ok((workspace, git, tree, note)) => super::AppEvent::WorkspaceOpened {
                request_id,
                repository_id,
                workspace,
                git,
                tree,
                note,
            },
            Err(error) => super::AppEvent::RuntimeFailed {
                request_id,
                repository_id,
                operation: RuntimeOperation::OpenWorkspace,
                error,
            },
        }
    }

    fn load_note(
        &self,
        request_id: super::RequestId,
        repository_id: Uuid,
        workspace: Workspace,
        path: PathBuf,
    ) -> super::AppEvent {
        let root = workspace.root().to_path_buf();
        let result = self.run_serialized(&root, || {
            workspace
                .resolve_note(&path)
                .map_err(RuntimeError::from)
                .and_then(|path| workspace.load_note(&path).map_err(RuntimeError::from))
        });
        match result {
            Ok(note) => super::AppEvent::NoteLoaded {
                request_id,
                repository_id,
                note,
            },
            Err(error) => super::AppEvent::RuntimeFailed {
                request_id,
                repository_id,
                operation: RuntimeOperation::LoadNote,
                error,
            },
        }
    }

    fn apply_and_commit(
        &self,
        origin: MutationOrigin,
        workspace: Workspace,
        git: GitRepo,
        operation: FileOperation,
        intent: CommitIntent,
    ) -> super::AppEvent {
        let MutationOrigin {
            mutation_id,
            repository_id,
            repository_root,
        } = origin;
        let root = workspace.root().to_path_buf();
        self.run_serialized(&root, || {
            match apply_and_commit(&workspace, &git, operation, intent) {
                Ok(MutationCommitOutcome::Applied { file, commit }) => {
                    super::AppEvent::MutationApplied {
                        mutation_id,
                        repository_id,
                        repository_root,
                        file,
                        commit,
                        tree: workspace.tree(),
                    }
                }
                Ok(MutationCommitOutcome::SavedCommitFailed { file, error }) => {
                    super::AppEvent::MutationSavedCommitFailed {
                        mutation_id,
                        repository_id,
                        repository_root,
                        file,
                        error,
                        tree: workspace.tree(),
                    }
                }
                Err(MutationCommitError::File(FileError::ExternalModification { path })) => {
                    super::AppEvent::MutationConflict {
                        mutation_id,
                        repository_id,
                        repository_root,
                        conflict: super::ExternalConflict::Modified { path },
                    }
                }
                Err(MutationCommitError::File(FileError::ExternalDeletion { path })) => {
                    super::AppEvent::MutationConflict {
                        mutation_id,
                        repository_id,
                        repository_root,
                        conflict: super::ExternalConflict::Deleted { path },
                    }
                }
                Err(error) => super::AppEvent::MutationFailed {
                    mutation_id,
                    repository_id,
                    repository_root,
                    error,
                },
            }
        })
    }

    fn retry_commit(
        &self,
        origin: MutationOrigin,
        git: GitRepo,
        intent: CommitIntent,
    ) -> super::AppEvent {
        let MutationOrigin {
            mutation_id,
            repository_id,
            repository_root,
        } = origin;
        let root = git.root().to_path_buf();
        self.run_serialized(&root, || match git.commit_all(intent) {
            Ok(commit) => super::AppEvent::CommitRetryApplied {
                mutation_id,
                repository_id,
                repository_root,
                commit,
            },
            Err(error) => super::AppEvent::CommitRetryFailed {
                mutation_id,
                repository_id,
                repository_root,
                error,
            },
        })
    }

    fn run_serialized<T>(&self, repository_root: &Path, operation: impl FnOnce() -> T) -> T {
        self.run_for_root(repository_root, operation)
    }

    pub(crate) fn run_for_root<T>(
        &self,
        repository_root: &Path,
        operation: impl FnOnce() -> T,
    ) -> T {
        let repository_lock = {
            let mut locks = self
                .repository_locks
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            Arc::clone(
                locks
                    .entry(repository_root.to_path_buf())
                    .or_insert_with(|| Arc::new(Mutex::new(()))),
            )
        };
        let _guard = repository_lock
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        operation()
    }
}

#[cfg(test)]
mod tests {
    use std::{
        path::Path,
        sync::mpsc::{self, RecvTimeoutError},
        thread,
        time::Duration,
    };

    use super::EffectExecutor;

    #[test]
    fn same_repository_operations_cannot_overlap() {
        let executor = EffectExecutor::default();
        let first = executor.clone();
        let (first_entered_tx, first_entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let first_thread = thread::spawn(move || {
            first.run_serialized(Path::new("/repo/a"), || {
                first_entered_tx.send(()).unwrap();
                release_rx.recv().unwrap();
            });
        });
        first_entered_rx.recv().unwrap();

        let second = executor.clone();
        let (second_started_tx, second_started_rx) = mpsc::channel();
        let (second_entered_tx, second_entered_rx) = mpsc::channel();
        let second_thread = thread::spawn(move || {
            second_started_tx.send(()).unwrap();
            second.run_serialized(Path::new("/repo/a"), || {
                second_entered_tx.send(()).unwrap();
            });
        });
        second_started_rx.recv().unwrap();

        assert_eq!(
            second_entered_rx.recv_timeout(Duration::from_millis(100)),
            Err(RecvTimeoutError::Timeout)
        );
        release_tx.send(()).unwrap();
        second_entered_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap();
        first_thread.join().unwrap();
        second_thread.join().unwrap();
    }

    #[test]
    fn different_repository_operations_use_independent_locks() {
        let executor = EffectExecutor::default();
        let first = executor.clone();
        let (first_entered_tx, first_entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let first_thread = thread::spawn(move || {
            first.run_serialized(Path::new("/repo/a"), || {
                first_entered_tx.send(()).unwrap();
                release_rx.recv().unwrap();
            });
        });
        first_entered_rx.recv().unwrap();

        let second = executor.clone();
        let (second_entered_tx, second_entered_rx) = mpsc::channel();
        let second_thread = thread::spawn(move || {
            second.run_serialized(Path::new("/repo/b"), || {
                second_entered_tx.send(()).unwrap();
            });
        });

        second_entered_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap();
        release_tx.send(()).unwrap();
        first_thread.join().unwrap();
        second_thread.join().unwrap();
    }
}
