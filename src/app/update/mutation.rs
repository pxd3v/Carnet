use std::path::PathBuf;

use uuid::Uuid;

use crate::{
    git::{CommitOutcome, GitError, MutationCommitError},
    workspace::{FileOutcome, TreeEntry},
};

use super::{clamp_tree_selection, rebase_path, select_tree_path};
use crate::app::{
    App, AppEffect, CommitStatus, Dialog, FailureKind, MutationId, PendingMutation,
    PendingMutationKind, RuntimeOperation, SavedCommitFailure, Screen, UnresolvedFailure,
};

struct FilesystemReconciliation {
    editor_has_newer_edits: bool,
    note_to_load: Option<PathBuf>,
    tree_error: Option<String>,
}

impl App {
    pub(super) fn handle_mutation_failed(
        &mut self,
        mutation_id: MutationId,
        repository_id: Uuid,
        repository_root: PathBuf,
        error: MutationCommitError,
    ) -> Vec<AppEffect> {
        if !self.mutation_result_is_current(mutation_id, repository_id, &repository_root) {
            return Vec::new();
        }
        self.pending_mutation = None;
        let kind = match error {
            MutationCommitError::File(_) => FailureKind::Write,
            MutationCommitError::WorkspaceMismatch
            | MutationCommitError::RepositoryMismatch
            | MutationCommitError::Runtime { .. } => FailureKind::Runtime,
        };
        let message = error.to_string();
        let failure = UnresolvedFailure {
            kind,
            message: message.clone(),
        };
        match kind {
            FailureKind::Write => self.failures.write = Some(failure),
            FailureKind::Runtime => self.record_unscoped_runtime_failure(
                repository_id,
                RuntimeOperation::Mutation,
                message.clone(),
            ),
            FailureKind::Git => unreachable!("mutation errors are write or runtime"),
        }
        self.status.commit = CommitStatus::Idle;
        self.status.message = Some(message.clone());
        self.dialog = Some(Dialog::Failure { kind, message });
        Vec::new()
    }

    pub(super) fn handle_commit_retry_failed(
        &mut self,
        mutation_id: MutationId,
        repository_id: Uuid,
        repository_root: PathBuf,
        error: GitError,
    ) -> Vec<AppEffect> {
        if !self.mutation_result_is_current(mutation_id, repository_id, &repository_root)
            || !matches!(
                self.pending_mutation.as_ref().map(|pending| pending.kind),
                Some(PendingMutationKind::RetryCommit)
            )
        {
            return Vec::new();
        }
        let pending = self.pending_mutation.take().expect("checked above");
        let message = error.to_string();
        self.saved_commit_failure = Some(SavedCommitFailure {
            repository_id,
            intent: pending.intent,
            message: message.clone(),
        });
        self.failures.git = Some(UnresolvedFailure {
            kind: FailureKind::Git,
            message: message.clone(),
        });
        self.status.commit = CommitStatus::SavedCommitFailed {
            message: message.clone(),
        };
        self.status.message = Some(message.clone());
        self.dialog = Some(Dialog::SavedCommitFailed { message });
        Vec::new()
    }

    pub(super) fn handle_commit_retry_applied(
        &mut self,
        mutation_id: MutationId,
        repository_id: Uuid,
        repository_root: PathBuf,
        commit: CommitOutcome,
    ) -> Vec<AppEffect> {
        if !self.mutation_result_is_current(mutation_id, repository_id, &repository_root)
            || !matches!(
                self.pending_mutation.as_ref().map(|pending| pending.kind),
                Some(PendingMutationKind::RetryCommit)
            )
        {
            return Vec::new();
        }
        self.pending_mutation = None;
        self.saved_commit_failure = None;
        self.failures.git = None;
        if matches!(self.dialog, Some(Dialog::SavedCommitFailed { .. })) {
            self.dialog = None;
        }
        self.status.commit = match commit {
            CommitOutcome::Committed { revision } => CommitStatus::Committed { revision },
            CommitOutcome::NoChanges => CommitStatus::NoChanges,
        };
        self.status.message = None;
        if self.pending_intent.is_some()
            && self
                .workspace_editor_mut()
                .is_some_and(|editor| editor.is_dirty())
        {
            self.dialog = Some(Dialog::DirtyNavigation);
            return Vec::new();
        }
        let intent = self.pending_intent.take();
        intent
            .map(|intent| self.perform_intent(intent))
            .unwrap_or_default()
    }

    pub(super) fn handle_saved_commit_failed(
        &mut self,
        mutation_id: MutationId,
        repository_id: Uuid,
        repository_root: PathBuf,
        file: FileOutcome,
        error: GitError,
        tree: Result<Vec<TreeEntry>, crate::workspace::FileError>,
    ) -> Vec<AppEffect> {
        if !self.mutation_result_is_current(mutation_id, repository_id, &repository_root) {
            return Vec::new();
        }
        let pending = self.pending_mutation.take().expect("checked above");
        let reconciliation = self.reconcile_filesystem_outcome(repository_id, &pending, file, tree);
        let message = error.to_string();
        self.saved_commit_failure = Some(SavedCommitFailure {
            repository_id,
            intent: pending.intent,
            message: message.clone(),
        });
        self.failures.git = Some(UnresolvedFailure {
            kind: FailureKind::Git,
            message: message.clone(),
        });
        self.status.commit = CommitStatus::SavedCommitFailed {
            message: message.clone(),
        };
        self.status.message = Some(message.clone());
        self.dialog = Some(Dialog::SavedCommitFailed { message });
        reconciliation
            .note_to_load
            .map(|path| self.request_note_load(path))
            .unwrap_or_default()
    }

    pub(super) fn handle_mutation_applied(
        &mut self,
        mutation_id: MutationId,
        repository_id: Uuid,
        repository_root: PathBuf,
        file: FileOutcome,
        commit: CommitOutcome,
        tree: Result<Vec<TreeEntry>, crate::workspace::FileError>,
    ) -> Vec<AppEffect> {
        if !self.mutation_result_is_current(mutation_id, repository_id, &repository_root) {
            return Vec::new();
        }
        let pending = self.pending_mutation.take().expect("checked above");
        let reconciliation = self.reconcile_filesystem_outcome(repository_id, &pending, file, tree);
        self.status.commit = match commit {
            CommitOutcome::Committed { revision } => CommitStatus::Committed { revision },
            CommitOutcome::NoChanges => CommitStatus::NoChanges,
        };
        self.status.message = None;
        if reconciliation.tree_error.is_some() {
            return reconciliation
                .note_to_load
                .map(|path| self.request_note_load(path))
                .unwrap_or_default();
        }
        if matches!(pending.kind, PendingMutationKind::Save { .. }) {
            if reconciliation.editor_has_newer_edits {
                if self.pending_intent.is_some() {
                    self.dialog = Some(Dialog::DirtyNavigation);
                }
                return Vec::new();
            }
            let intent = self.pending_intent.take();
            return intent
                .map(|intent| self.perform_intent(intent))
                .unwrap_or_default();
        }
        reconciliation
            .note_to_load
            .map(|path| self.request_note_load(path))
            .unwrap_or_default()
    }

    fn reconcile_filesystem_outcome(
        &mut self,
        repository_id: Uuid,
        pending: &PendingMutation,
        file: FileOutcome,
        tree: Result<Vec<TreeEntry>, crate::workspace::FileError>,
    ) -> FilesystemReconciliation {
        let tree_error = tree.as_ref().err().map(ToString::to_string);
        let tree_refreshed = tree.is_ok();
        let mut editor_has_newer_edits = false;
        let mut note_to_load = None;
        if let Screen::Workspace(workspace) = &mut self.screen {
            if let Ok(tree) = tree {
                workspace.tree = tree;
            }
            match file {
                FileOutcome::Saved(note) => {
                    if let Some(editor) = &mut workspace.editor {
                        editor_has_newer_edits = pending
                            .save
                            .as_ref()
                            .is_some_and(|save| editor.text() != save.snapshot);
                        editor.accept_saved(note);
                        editor_has_newer_edits |= editor.is_dirty();
                    }
                    clamp_tree_selection(workspace);
                }
                FileOutcome::CreatedFile(note) => {
                    let path = note.relative().to_path_buf();
                    select_tree_path(workspace, &path);
                    note_to_load = Some(path);
                }
                FileOutcome::CreatedFolder(path) => select_tree_path(workspace, &path),
                FileOutcome::Renamed { from, to } | FileOutcome::Moved { from, to } => {
                    select_tree_path(workspace, &to);
                    if let Some(rebased) = workspace
                        .current_note
                        .as_deref()
                        .and_then(|path| rebase_path(path, &from, &to))
                    {
                        note_to_load = Some(rebased);
                    }
                }
                FileOutcome::Deleted(path) => {
                    if workspace
                        .current_note
                        .as_deref()
                        .is_some_and(|note| note.starts_with(&path))
                    {
                        workspace.current_note = None;
                        workspace.editor = None;
                        workspace.editor_instance_id = None;
                        workspace.editor_revision = 0;
                    }
                    clamp_tree_selection(workspace);
                }
            }
        }
        self.failures.write = None;
        self.clear_runtime_failures(repository_id, RuntimeOperation::Mutation);
        if tree_refreshed {
            self.clear_runtime_failures(repository_id, RuntimeOperation::RefreshTree);
        } else if let Some(message) = tree_error.clone() {
            self.record_unscoped_runtime_failure(
                repository_id,
                RuntimeOperation::RefreshTree,
                message,
            );
        }
        FilesystemReconciliation {
            editor_has_newer_edits,
            note_to_load,
            tree_error,
        }
    }
}
