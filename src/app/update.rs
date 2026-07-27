use uuid::Uuid;

use crate::{
    catalog::CatalogError,
    editor::{ClipboardError, Editor, EditorCommand},
    git::{CommitIntent, CommitOutcome, GitError, GitRepo, MutationCommitError},
    workspace::{FileOperation, FileOutcome, LoadedNote, TreeEntry, TreeEntryKind, Workspace},
};

use super::{
    App, AppEffect, AppExitStatus, CommitStatus, DefaultChoiceState, Dialog, ExternalConflict,
    FailureKind, FileActionKind, FileMutationAction, Focus, NavigationAction, OverlayState,
    PendingIntent, PendingMutation, PendingMutationKind, PendingRequest, PendingSave, RequestId,
    RuntimeFailure, SavedCommitFailure, Screen, UnresolvedFailure, WorkspaceState,
};
use super::{RuntimeError, RuntimeOperation};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HomeAction {
    Up,
    Down,
    OpenSelected,
    ChooseSelectedAsDefault,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GlobalAction {
    Save,
    Find,
    QuickOpen,
    ToggleSidebar,
    Undo,
    Redo,
    Copy,
    Cut,
    Paste,
    SelectAll,
    Quit,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DirtyChoice {
    Cancel,
    Discard,
    Save,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConflictChoice {
    Cancel,
    Overwrite,
    Reload,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TreeAction {
    Up,
    Down,
    Left,
    Right,
    Open,
    NewFile,
    NewFolder,
    Rename,
    Move,
    Delete,
    Escape,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AppAction {
    Home(HomeAction),
    Global(GlobalAction),
    Editor(EditorCommand),
    Navigate(NavigationAction),
    Focus(Focus),
    Tree(TreeAction),
    Dismiss,
    SubmitFileAction(std::path::PathBuf),
    ConfirmDelete,
    SetSidebarOverlayIntent(bool),
}

#[derive(Debug)]
pub enum AppEvent {
    Action(AppAction),
    ClipboardRead(Result<String, ClipboardError>),
    ClipboardWritten(Result<(), ClipboardError>),
    DefaultRepositoryPersisted {
        repository_id: Uuid,
        result: Result<(), CatalogError>,
    },
    DirtyChoice(DirtyChoice),
    ConflictChoice(ConflictChoice),
    MutationApplied {
        repository_id: Uuid,
        file: FileOutcome,
        commit: CommitOutcome,
        tree: Result<Vec<TreeEntry>, crate::workspace::FileError>,
    },
    MutationConflict {
        repository_id: Uuid,
        conflict: ExternalConflict,
    },
    MutationSavedCommitFailed {
        repository_id: Uuid,
        file: FileOutcome,
        error: GitError,
        tree: Result<Vec<TreeEntry>, crate::workspace::FileError>,
    },
    MutationFailed {
        repository_id: Uuid,
        error: MutationCommitError,
    },
    CommitRetryApplied {
        repository_id: Uuid,
        commit: CommitOutcome,
    },
    CommitRetryFailed {
        repository_id: Uuid,
        error: GitError,
    },
    WorkspaceOpened {
        request_id: RequestId,
        repository_id: Uuid,
        workspace: Workspace,
        git: GitRepo,
        tree: Vec<TreeEntry>,
        note: Option<LoadedNote>,
    },
    NoteLoaded {
        request_id: RequestId,
        repository_id: Uuid,
        note: LoadedNote,
    },
    RuntimeFailed {
        request_id: RequestId,
        repository_id: Uuid,
        operation: RuntimeOperation,
        error: RuntimeError,
    },
}

impl App {
    pub fn update(&mut self, event: AppEvent) -> Vec<AppEffect> {
        if self.editor_commands_suppressed() && is_editor_command_event(&event) {
            return Vec::new();
        }
        match event {
            AppEvent::DefaultRepositoryPersisted {
                repository_id,
                result,
            } => {
                if self.home.default_repository != Some(repository_id) {
                    return Vec::new();
                }
                match result {
                    Ok(()) => {
                        self.failures.catalog = None;
                        Vec::new()
                    }
                    Err(error) => {
                        self.record_outer_failure(FailureKind::Runtime, error.to_string(), false);
                        Vec::new()
                    }
                }
            }
            AppEvent::ClipboardWritten(Ok(())) => {
                self.failures.clipboard = None;
                Vec::new()
            }
            AppEvent::ClipboardWritten(Err(error)) => {
                self.record_outer_failure(FailureKind::Runtime, error.to_string(), true);
                Vec::new()
            }
            AppEvent::Action(AppAction::Home(HomeAction::Up)) => {
                if let Some(selected) = self.home.selected {
                    self.home.selected = Some(selected.saturating_sub(1));
                }
                Vec::new()
            }
            AppEvent::RuntimeFailed {
                request_id,
                repository_id,
                operation,
                error,
            } => {
                let current = match operation {
                    RuntimeOperation::OpenWorkspace => matches!(
                        self.pending_request,
                        Some(PendingRequest::OpenWorkspace {
                            request_id: pending_request,
                            repository_id: pending_repository,
                        }) if pending_request == request_id && pending_repository == repository_id
                    ),
                    RuntimeOperation::LoadNote => matches!(
                        self.pending_request,
                        Some(PendingRequest::LoadNote {
                            request_id: pending_request,
                            repository_id: pending_repository,
                            ..
                        }) if pending_request == request_id && pending_repository == repository_id
                    ),
                    RuntimeOperation::Mutation | RuntimeOperation::RefreshTree => false,
                };
                if !current {
                    return Vec::new();
                }
                self.pending_request = None;
                self.record_request_failure(
                    request_id,
                    repository_id,
                    operation,
                    error.to_string(),
                );
                Vec::new()
            }
            AppEvent::Action(AppAction::ConfirmDelete) => self.confirm_delete(),
            AppEvent::Action(AppAction::SubmitFileAction(path)) => self.submit_file_action(path),
            AppEvent::Action(AppAction::Dismiss) => {
                self.dialog = None;
                self.overlay = OverlayState::None;
                Vec::new()
            }
            AppEvent::Action(AppAction::Focus(focus)) => {
                if let Screen::Workspace(workspace) = &mut self.screen {
                    workspace.focus = focus;
                }
                Vec::new()
            }
            AppEvent::Action(AppAction::Tree(action)) => self.tree_action(action),
            AppEvent::MutationFailed {
                repository_id,
                error,
            } => {
                if self
                    .pending_mutation
                    .as_ref()
                    .map(|pending| pending.repository_id)
                    != Some(repository_id)
                {
                    return Vec::new();
                }
                self.pending_mutation = None;
                let kind = match error {
                    MutationCommitError::File(_) => FailureKind::Write,
                    MutationCommitError::WorkspaceMismatch
                    | MutationCommitError::RepositoryMismatch => FailureKind::Runtime,
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
            AppEvent::CommitRetryFailed {
                repository_id,
                error,
            } => {
                if !matches!(
                    self.pending_mutation.as_ref(),
                    Some(PendingMutation {
                        repository_id: pending_repository,
                        kind: PendingMutationKind::RetryCommit,
                        ..
                    }) if *pending_repository == repository_id
                ) {
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
            AppEvent::CommitRetryApplied {
                repository_id,
                commit,
            } => {
                if !matches!(
                    self.pending_mutation.as_ref(),
                    Some(PendingMutation {
                        repository_id: pending_repository,
                        kind: PendingMutationKind::RetryCommit,
                        ..
                    }) if *pending_repository == repository_id
                ) {
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
            AppEvent::MutationSavedCommitFailed {
                repository_id,
                file,
                error,
                tree,
            } => {
                if self
                    .pending_mutation
                    .as_ref()
                    .map(|pending| pending.repository_id)
                    != Some(repository_id)
                {
                    return Vec::new();
                }
                let pending = self.pending_mutation.take().expect("checked above");
                if let Screen::Workspace(workspace) = &mut self.screen {
                    if let Ok(tree) = tree {
                        workspace.tree = tree;
                    }
                    if let (Some(editor), FileOutcome::Saved(note)) = (&mut workspace.editor, file)
                    {
                        editor.accept_saved(note);
                    }
                }
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
            AppEvent::NoteLoaded {
                request_id,
                repository_id,
                note,
            } => {
                if !matches!(
                    self.pending_request.as_ref(),
                    Some(PendingRequest::LoadNote {
                        request_id: pending_request,
                        repository_id: pending_repository,
                        path,
                    }) if *pending_request == request_id
                        && *pending_repository == repository_id
                        && path.as_path() == note.path().relative()
                ) {
                    return Vec::new();
                }
                let Screen::Workspace(workspace) = &mut self.screen else {
                    return Vec::new();
                };
                if workspace.repository.id != repository_id {
                    return Vec::new();
                }
                workspace.current_note = Some(note.path().relative().to_path_buf());
                workspace.editor = Some(Editor::from_loaded(note));
                self.pending_request = None;
                self.clear_runtime_failures(repository_id, RuntimeOperation::LoadNote);
                Vec::new()
            }
            AppEvent::ConflictChoice(ConflictChoice::Reload) => {
                self.dialog = None;
                self.pending_intent = None;
                let path = match &self.screen {
                    Screen::Workspace(workspace) => workspace.current_note.clone(),
                    Screen::Home => None,
                };
                path.map(|path| self.perform_navigation(NavigationAction::Note(path)))
                    .unwrap_or_default()
            }
            AppEvent::ConflictChoice(ConflictChoice::Overwrite) => {
                self.dialog = None;
                self.save(true)
            }
            AppEvent::ConflictChoice(ConflictChoice::Cancel) => {
                self.dialog = None;
                self.pending_intent = None;
                Vec::new()
            }
            AppEvent::MutationConflict {
                repository_id,
                conflict,
            } => {
                if self
                    .pending_mutation
                    .as_ref()
                    .map(|pending| pending.repository_id)
                    != Some(repository_id)
                {
                    return Vec::new();
                }
                self.pending_mutation = None;
                self.status.commit = CommitStatus::Idle;
                self.status.message = Some("file changed outside Carnet".into());
                self.dialog = Some(Dialog::ExternalConflict(conflict));
                Vec::new()
            }
            AppEvent::Action(AppAction::Global(GlobalAction::Quit)) => self.update(
                AppEvent::Action(AppAction::Navigate(NavigationAction::Quit)),
            ),
            AppEvent::MutationApplied {
                repository_id,
                file,
                commit,
                tree,
            } => {
                let Some(pending) = self.pending_mutation.as_ref() else {
                    return Vec::new();
                };
                if pending.repository_id != repository_id {
                    return Vec::new();
                }
                let pending = self.pending_mutation.take().expect("checked above");
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
                        FileOutcome::CreatedFolder(path) => {
                            select_tree_path(workspace, &path);
                        }
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
                            }
                            clamp_tree_selection(workspace);
                        }
                    }
                }
                self.status.commit = match commit {
                    CommitOutcome::Committed { revision } => CommitStatus::Committed { revision },
                    CommitOutcome::NoChanges => CommitStatus::NoChanges,
                };
                self.failures.write = None;
                self.clear_runtime_failures(repository_id, RuntimeOperation::Mutation);
                if tree_refreshed {
                    self.clear_runtime_failures(repository_id, RuntimeOperation::RefreshTree);
                }
                self.status.message = None;
                if let Some(message) = tree_error {
                    self.record_unscoped_runtime_failure(
                        repository_id,
                        RuntimeOperation::RefreshTree,
                        message,
                    );
                    return note_to_load
                        .map(|path| self.request_note_load(path))
                        .unwrap_or_default();
                }
                if matches!(pending.kind, PendingMutationKind::Save { .. }) {
                    if editor_has_newer_edits {
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
                note_to_load
                    .map(|path| self.request_note_load(path))
                    .unwrap_or_default()
            }
            AppEvent::DirtyChoice(DirtyChoice::Save) => {
                if self.pending_mutation.is_some() {
                    return Vec::new();
                }
                self.dialog = None;
                self.global_save()
            }
            AppEvent::DirtyChoice(DirtyChoice::Discard) => {
                if self.pending_mutation.is_some() {
                    return Vec::new();
                }
                let intent = self.pending_intent.take();
                self.discard_editor_changes();
                self.dialog = None;
                intent
                    .map(|intent| self.perform_intent(intent))
                    .unwrap_or_default()
            }
            AppEvent::DirtyChoice(DirtyChoice::Cancel) => {
                if self.pending_mutation.is_some() {
                    return Vec::new();
                }
                self.pending_intent = None;
                self.dialog = None;
                Vec::new()
            }
            AppEvent::Action(AppAction::Navigate(target)) => {
                if self.pending_mutation.is_some() {
                    return Vec::new();
                }
                if self
                    .workspace_editor_mut()
                    .is_some_and(|editor| editor.is_dirty())
                {
                    self.pending_intent = Some(PendingIntent::Navigation(target));
                    self.dialog = Some(Dialog::DirtyNavigation);
                    Vec::new()
                } else {
                    self.perform_navigation(target)
                }
            }
            AppEvent::Action(AppAction::Global(GlobalAction::Save)) => self.global_save(),
            AppEvent::ClipboardRead(Ok(text)) => {
                self.failures.clipboard = None;
                if let Some(editor) = self.workspace_editor_mut() {
                    editor.apply(EditorCommand::BracketedPaste(text));
                }
                Vec::new()
            }
            AppEvent::ClipboardRead(Err(error)) => {
                self.record_outer_failure(FailureKind::Runtime, error.to_string(), true);
                Vec::new()
            }
            AppEvent::Action(AppAction::Editor(EditorCommand::Copy)) => {
                self.update(AppEvent::Action(AppAction::Global(GlobalAction::Copy)))
            }
            AppEvent::Action(AppAction::Editor(EditorCommand::Cut)) => {
                self.update(AppEvent::Action(AppAction::Global(GlobalAction::Cut)))
            }
            AppEvent::Action(AppAction::Editor(EditorCommand::Paste)) => {
                self.update(AppEvent::Action(AppAction::Global(GlobalAction::Paste)))
            }
            AppEvent::Action(AppAction::Editor(command)) => {
                if let Some(editor) = self.workspace_editor_mut() {
                    editor.apply(command);
                }
                Vec::new()
            }
            AppEvent::Action(AppAction::Global(GlobalAction::Undo)) => {
                if let Some(editor) = self.workspace_editor_mut() {
                    editor.apply(EditorCommand::Undo);
                }
                Vec::new()
            }
            AppEvent::Action(AppAction::Global(GlobalAction::Redo)) => {
                if let Some(editor) = self.workspace_editor_mut() {
                    editor.apply(EditorCommand::Redo);
                }
                Vec::new()
            }
            AppEvent::Action(AppAction::Global(GlobalAction::SelectAll)) => {
                if let Some(editor) = self.workspace_editor_mut() {
                    editor.apply(EditorCommand::SelectAll);
                }
                Vec::new()
            }
            AppEvent::Action(AppAction::Global(GlobalAction::Copy)) => self
                .workspace_editor_mut()
                .and_then(|editor| editor.selected_text())
                .map(|text| vec![AppEffect::WriteClipboard { text }])
                .unwrap_or_default(),
            AppEvent::Action(AppAction::Global(GlobalAction::Cut)) => {
                let Some(editor) = self.workspace_editor_mut() else {
                    return Vec::new();
                };
                let Some(text) = editor.selected_text() else {
                    return Vec::new();
                };
                editor.apply(EditorCommand::Insert(String::new()));
                vec![AppEffect::WriteClipboard { text }]
            }
            AppEvent::Action(AppAction::Global(GlobalAction::Paste)) => {
                (self.workspace_editor_mut().is_some())
                    .then_some(AppEffect::ReadClipboard)
                    .into_iter()
                    .collect()
            }
            AppEvent::Action(AppAction::Global(GlobalAction::Find)) => {
                self.overlay = OverlayState::Search {
                    query: String::new(),
                };
                Vec::new()
            }
            AppEvent::Action(AppAction::Global(GlobalAction::QuickOpen)) => {
                self.overlay = OverlayState::QuickOpen {
                    query: String::new(),
                    selected: None,
                };
                Vec::new()
            }
            AppEvent::Action(AppAction::Global(GlobalAction::ToggleSidebar)) => {
                self.sidebar.visible = !self.sidebar.visible;
                Vec::new()
            }
            AppEvent::Action(AppAction::SetSidebarOverlayIntent(overlay_intent)) => {
                self.sidebar.overlay_intent = overlay_intent;
                Vec::new()
            }
            AppEvent::WorkspaceOpened {
                request_id,
                repository_id,
                workspace,
                git,
                tree,
                note,
            } => {
                if workspace.repo().id != repository_id
                    || !matches!(
                        self.pending_request,
                        Some(PendingRequest::OpenWorkspace {
                            request_id: pending_request,
                            repository_id: pending_repository,
                        }) if pending_request == request_id
                            && pending_repository == repository_id
                    )
                {
                    return Vec::new();
                }
                self.pending_request = None;
                let current_note = note
                    .as_ref()
                    .map(|note| note.path().relative().to_path_buf());
                let repository = workspace.repo().clone();
                let tree_selection = (!tree.is_empty()).then_some(0);
                self.screen = Screen::Workspace(Box::new(WorkspaceState {
                    repository,
                    workspace,
                    git,
                    tree,
                    current_note,
                    editor: note.map(Editor::from_loaded),
                    focus: Focus::Editor,
                    tree_selection,
                    expanded: Default::default(),
                }));
                if matches!(
                    self.home.default_choice,
                    DefaultChoiceState::ResumingPendingNote {
                        repository_id: expected,
                        ..
                    } if expected == repository_id
                ) {
                    self.home.pending_note = None;
                    self.home.default_choice = DefaultChoiceState::NotNeeded;
                }
                self.clear_runtime_failures(repository_id, RuntimeOperation::OpenWorkspace);
                Vec::new()
            }
            AppEvent::Action(AppAction::Home(HomeAction::Down)) => {
                if let Some(selected) = self.home.selected {
                    self.home.selected = Some(
                        selected
                            .saturating_add(1)
                            .min(self.home.repositories.len().saturating_sub(1)),
                    );
                }
                Vec::new()
            }
            AppEvent::Action(AppAction::Home(HomeAction::OpenSelected)) => {
                let Some(repository) = self
                    .home
                    .selected
                    .and_then(|selected| self.home.repositories.get(selected))
                    .cloned()
                else {
                    return Vec::new();
                };
                let note = self.home.pending_note.clone();
                if let Some(note) = &note {
                    self.home.default_choice = DefaultChoiceState::ResumingPendingNote {
                        repository_id: repository.id,
                        note: note.clone(),
                    };
                }
                self.request_open_workspace(repository, note)
            }
            AppEvent::Action(AppAction::Home(HomeAction::ChooseSelectedAsDefault)) => {
                if self.pending_mutation.is_some() {
                    return Vec::new();
                }
                let Some(repository) = self
                    .home
                    .selected
                    .and_then(|selected| self.home.repositories.get(selected))
                    .cloned()
                else {
                    return Vec::new();
                };
                let Some(note) = self.home.pending_note.clone() else {
                    return Vec::new();
                };
                self.home.default_repository = Some(repository.id);
                self.home.default_choice = super::DefaultChoiceState::ResumingPendingNote {
                    repository_id: repository.id,
                    note: note.clone(),
                };
                let mut effects = self.request_open_workspace(repository.clone(), Some(note));
                effects.insert(
                    0,
                    AppEffect::SetDefaultRepository {
                        repository_id: repository.id,
                    },
                );
                effects
            }
        }
    }

    fn workspace_editor_mut(&mut self) -> Option<&mut Editor> {
        let Screen::Workspace(workspace) = &mut self.screen else {
            return None;
        };
        workspace.editor.as_mut()
    }

    fn editor_commands_suppressed(&self) -> bool {
        self.pending_request.is_some()
            || self
                .pending_mutation
                .as_ref()
                .is_some_and(|pending| pending.reconciles_editor)
    }

    fn record_request_failure(
        &mut self,
        request_id: RequestId,
        repository_id: Uuid,
        operation: RuntimeOperation,
        message: String,
    ) {
        self.failures.runtime.retain(|failure| {
            failure.repository_id != repository_id || failure.operation != operation
        });
        self.failures.runtime.push(RuntimeFailure {
            request_id: Some(request_id),
            repository_id,
            operation,
            message: message.clone(),
        });
        self.status.message = Some(message.clone());
        self.dialog = Some(Dialog::Failure {
            kind: FailureKind::Runtime,
            message,
        });
    }

    fn record_unscoped_runtime_failure(
        &mut self,
        repository_id: Uuid,
        operation: RuntimeOperation,
        message: String,
    ) {
        self.failures.runtime.retain(|failure| {
            failure.repository_id != repository_id || failure.operation != operation
        });
        self.failures.runtime.push(RuntimeFailure {
            request_id: None,
            repository_id,
            operation,
            message: message.clone(),
        });
        self.status.message = Some(message.clone());
        self.dialog = Some(Dialog::Failure {
            kind: FailureKind::Runtime,
            message,
        });
    }

    fn clear_runtime_failures(&mut self, repository_id: Uuid, operation: RuntimeOperation) {
        self.failures.runtime.retain(|failure| {
            failure.repository_id != repository_id || failure.operation != operation
        });
    }

    fn record_outer_failure(&mut self, kind: FailureKind, message: String, clipboard: bool) {
        let failure = UnresolvedFailure {
            kind,
            message: message.clone(),
        };
        if clipboard {
            self.failures.clipboard = Some(failure);
        } else {
            self.failures.catalog = Some(failure);
        }
        self.status.message = Some(message.clone());
        self.dialog = Some(Dialog::Failure { kind, message });
    }

    fn save(&mut self, overwrite: bool) -> Vec<AppEffect> {
        if self.pending_mutation.is_some() {
            return Vec::new();
        }
        let Screen::Workspace(workspace) = &self.screen else {
            return Vec::new();
        };
        let Some(editor) = &workspace.editor else {
            return Vec::new();
        };
        if !editor.is_dirty() {
            return Vec::new();
        }
        let operation = editor.save_operation(overwrite);
        let (path, is_saved) = match &operation {
            FileOperation::Save { note, .. } => {
                (note.path().relative().to_path_buf(), note.is_saved())
            }
            _ => unreachable!("the editor only creates save operations"),
        };
        let intent = if is_saved {
            CommitIntent::Update(path)
        } else {
            CommitIntent::Create(path)
        };
        let repository_id = workspace.repository.id;
        let effect_workspace = workspace.workspace.clone();
        let effect_git = workspace.git.clone();
        let snapshot = editor.text();
        let generation = self.next_save_generation;
        self.next_save_generation = self
            .next_save_generation
            .checked_add(1)
            .expect("save generation overflow");
        self.pending_mutation = Some(PendingMutation {
            repository_id,
            kind: PendingMutationKind::Save { overwrite },
            intent: intent.clone(),
            save: Some(PendingSave {
                generation,
                snapshot,
            }),
            reconciles_editor: false,
        });
        self.status.commit = CommitStatus::Pending;
        vec![AppEffect::ApplyAndCommit {
            repository_id,
            workspace: effect_workspace,
            git: effect_git,
            operation: Box::new(operation),
            intent,
        }]
    }

    fn global_save(&mut self) -> Vec<AppEffect> {
        if self.pending_mutation.is_some() {
            return Vec::new();
        }
        if let Some(failure) = self.saved_commit_failure.clone() {
            let Screen::Workspace(workspace) = &self.screen else {
                return Vec::new();
            };
            if workspace.repository.id != failure.repository_id {
                return Vec::new();
            }
            self.pending_mutation = Some(PendingMutation {
                repository_id: failure.repository_id,
                kind: PendingMutationKind::RetryCommit,
                intent: failure.intent.clone(),
                save: None,
                reconciles_editor: false,
            });
            self.status.commit = CommitStatus::Pending;
            return vec![AppEffect::RetryCommit {
                repository_id: failure.repository_id,
                git: workspace.git.clone(),
                intent: failure.intent,
            }];
        }
        self.save(false)
    }

    fn perform_intent(&mut self, intent: PendingIntent) -> Vec<AppEffect> {
        match intent {
            PendingIntent::Navigation(target) => self.perform_navigation(target),
            PendingIntent::Mutation(action) => self.start_file_mutation(action),
        }
    }

    fn discard_editor_changes(&mut self) {
        if let Some(editor) = self.workspace_editor_mut() {
            editor.discard_changes();
        }
    }

    fn perform_navigation(&mut self, target: NavigationAction) -> Vec<AppEffect> {
        match target {
            NavigationAction::Home => {
                self.screen = Screen::Home;
                self.overlay = OverlayState::None;
                self.dialog = None;
                self.pending_request = None;
                Vec::new()
            }
            NavigationAction::Repository { repository, note } => {
                self.request_open_workspace(repository, note)
            }
            NavigationAction::Note(path) => self.request_note_load(path),
            NavigationAction::Quit => {
                self.quit.requested = true;
                self.quit.final_status = Some(if !self.failures.is_empty() {
                    AppExitStatus::Failure
                } else {
                    AppExitStatus::Success
                });
                Vec::new()
            }
        }
    }

    fn next_request_id(&mut self) -> RequestId {
        let request_id = RequestId(self.next_request_id);
        self.next_request_id = self
            .next_request_id
            .checked_add(1)
            .expect("application request ID overflow");
        request_id
    }

    fn request_open_workspace(
        &mut self,
        repository: crate::catalog::RepoEntry,
        note: Option<std::path::PathBuf>,
    ) -> Vec<AppEffect> {
        if self.pending_mutation.is_some() {
            return Vec::new();
        }
        let request_id = self.next_request_id();
        self.pending_request = Some(PendingRequest::OpenWorkspace {
            request_id,
            repository_id: repository.id,
        });
        vec![AppEffect::OpenWorkspace {
            request_id,
            repository,
            note,
        }]
    }

    fn request_note_load(&mut self, path: std::path::PathBuf) -> Vec<AppEffect> {
        let (repository_id, workspace) = match &self.screen {
            Screen::Workspace(workspace) => (workspace.repository.id, workspace.workspace.clone()),
            Screen::Home => return Vec::new(),
        };
        let request_id = self.next_request_id();
        self.pending_request = Some(PendingRequest::LoadNote {
            request_id,
            repository_id,
            path: path.clone(),
        });
        vec![AppEffect::LoadNote {
            request_id,
            repository_id,
            workspace,
            path,
        }]
    }

    fn tree_action(&mut self, action: TreeAction) -> Vec<AppEffect> {
        if self.pending_mutation.is_some()
            && matches!(
                action,
                TreeAction::NewFile
                    | TreeAction::NewFolder
                    | TreeAction::Rename
                    | TreeAction::Move
                    | TreeAction::Delete
            )
        {
            return Vec::new();
        }
        let (focus, entries, selected) = match &self.screen {
            Screen::Workspace(workspace) => (
                workspace.focus,
                visible_tree(&workspace.tree, &workspace.expanded),
                workspace.tree_selection,
            ),
            Screen::Home => return Vec::new(),
        };
        if focus != Focus::Tree {
            return Vec::new();
        }
        if action == TreeAction::Escape {
            if let Screen::Workspace(workspace) = &mut self.screen {
                workspace.focus = Focus::Editor;
            }
            if self.sidebar.overlay_intent {
                self.sidebar.visible = false;
            }
            return Vec::new();
        }
        if entries.is_empty() {
            let kind = match action {
                TreeAction::NewFile => Some(FileActionKind::NewFile),
                TreeAction::NewFolder => Some(FileActionKind::NewFolder),
                _ => None,
            };
            if let Some(kind) = kind {
                self.dialog = Some(Dialog::FileAction { kind, target: None });
            }
            return Vec::new();
        }
        let Some(selected) = selected.filter(|selected| *selected < entries.len()) else {
            return Vec::new();
        };
        match action {
            TreeAction::Up => {
                if let Screen::Workspace(workspace) = &mut self.screen {
                    workspace.tree_selection = Some(selected.saturating_sub(1));
                }
            }
            TreeAction::Down => {
                if let Screen::Workspace(workspace) = &mut self.screen {
                    workspace.tree_selection = Some((selected + 1).min(entries.len() - 1));
                }
            }
            TreeAction::Right => {
                if entries[selected].kind == TreeEntryKind::Directory
                    && let Screen::Workspace(workspace) = &mut self.screen
                {
                    workspace.expanded.insert(entries[selected].path.clone());
                }
            }
            TreeAction::Left => {
                if let Screen::Workspace(workspace) = &mut self.screen {
                    if entries[selected].kind == TreeEntryKind::Directory
                        && workspace.expanded.remove(&entries[selected].path)
                    {
                        return Vec::new();
                    }
                    if let Some(parent) = &entries[selected].parent
                        && let Some(parent_index) = entries
                            .iter()
                            .position(|entry| entry.path.as_path() == parent.as_path())
                    {
                        workspace.tree_selection = Some(parent_index);
                    }
                }
            }
            TreeAction::Open => {
                if entries[selected].kind == TreeEntryKind::Directory {
                    if let Screen::Workspace(workspace) = &mut self.screen
                        && !workspace.expanded.remove(&entries[selected].path)
                    {
                        workspace.expanded.insert(entries[selected].path.clone());
                    }
                } else if entries[selected].enabled {
                    return self.update(AppEvent::Action(AppAction::Navigate(
                        NavigationAction::Note(entries[selected].path.clone()),
                    )));
                }
            }
            TreeAction::NewFile | TreeAction::NewFolder | TreeAction::Rename | TreeAction::Move => {
                let kind = match action {
                    TreeAction::NewFile => FileActionKind::NewFile,
                    TreeAction::NewFolder => FileActionKind::NewFolder,
                    TreeAction::Rename => FileActionKind::Rename,
                    TreeAction::Move => FileActionKind::Move,
                    _ => unreachable!(),
                };
                self.dialog = Some(Dialog::FileAction {
                    kind,
                    target: Some(entries[selected].path.clone()),
                });
            }
            TreeAction::Delete => {
                self.dialog = Some(Dialog::ConfirmDelete {
                    path: entries[selected].path.clone(),
                });
            }
            TreeAction::Escape => unreachable!("handled before selection"),
        }
        Vec::new()
    }

    fn submit_file_action(&mut self, path: std::path::PathBuf) -> Vec<AppEffect> {
        if self.pending_mutation.is_some() {
            return Vec::new();
        }
        let Some(Dialog::FileAction { kind, target }) = self.dialog.take() else {
            return Vec::new();
        };
        let action = match kind {
            FileActionKind::NewFile => FileMutationAction::CreateFile { path },
            FileActionKind::NewFolder => FileMutationAction::CreateFolder { path },
            FileActionKind::Rename => {
                let Some(from) = target else {
                    return Vec::new();
                };
                FileMutationAction::Rename { from, to: path }
            }
            FileActionKind::Move => {
                let Some(from) = target else {
                    return Vec::new();
                };
                FileMutationAction::Move { from, to: path }
            }
        };
        self.request_file_mutation(action)
    }

    fn confirm_delete(&mut self) -> Vec<AppEffect> {
        if self.pending_mutation.is_some() {
            return Vec::new();
        }
        let Some(Dialog::ConfirmDelete { path }) = self.dialog.take() else {
            return Vec::new();
        };
        self.request_file_mutation(FileMutationAction::Delete { path })
    }

    fn request_file_mutation(&mut self, action: FileMutationAction) -> Vec<AppEffect> {
        if self.pending_mutation.is_some() {
            return Vec::new();
        }
        let should_guard = match &self.screen {
            Screen::Workspace(workspace) => {
                workspace
                    .editor
                    .as_ref()
                    .is_some_and(|editor| editor.is_dirty())
                    && mutation_reconciles_editor(workspace, &action)
            }
            Screen::Home => false,
        };
        if should_guard {
            self.pending_intent = Some(PendingIntent::Mutation(action));
            self.dialog = Some(Dialog::DirtyNavigation);
            return Vec::new();
        }
        self.start_file_mutation(action)
    }

    fn start_file_mutation(&mut self, action: FileMutationAction) -> Vec<AppEffect> {
        if self.pending_mutation.is_some() {
            return Vec::new();
        }
        let Screen::Workspace(workspace) = &self.screen else {
            return Vec::new();
        };
        let reconciles_editor = mutation_reconciles_editor(workspace, &action);
        let effect_workspace = workspace.workspace.clone();
        let (kind, operation, intent) = match action {
            FileMutationAction::CreateFile { path } => (
                PendingMutationKind::File(FileActionKind::NewFile),
                FileOperation::CreateFile {
                    workspace: effect_workspace.clone(),
                    path: path.clone(),
                },
                CommitIntent::Create(path),
            ),
            FileMutationAction::CreateFolder { path } => (
                PendingMutationKind::File(FileActionKind::NewFolder),
                FileOperation::CreateFolder {
                    workspace: effect_workspace.clone(),
                    path: path.clone(),
                },
                CommitIntent::Create(path),
            ),
            FileMutationAction::Rename { from, to } => (
                PendingMutationKind::File(FileActionKind::Rename),
                FileOperation::Rename {
                    workspace: effect_workspace.clone(),
                    from: from.clone(),
                    to: to.clone(),
                },
                CommitIntent::Move { from, to },
            ),
            FileMutationAction::Move { from, to } => (
                PendingMutationKind::File(FileActionKind::Move),
                FileOperation::Move {
                    workspace: effect_workspace.clone(),
                    from: from.clone(),
                    to: to.clone(),
                },
                CommitIntent::Move { from, to },
            ),
            FileMutationAction::Delete { path } => (
                PendingMutationKind::Delete,
                FileOperation::Delete {
                    workspace: effect_workspace.clone(),
                    path: path.clone(),
                    confirmed: true,
                },
                CommitIntent::Delete(path),
            ),
        };
        let repository_id = workspace.repository.id;
        self.pending_mutation = Some(PendingMutation {
            repository_id,
            kind,
            intent: intent.clone(),
            save: None,
            reconciles_editor,
        });
        self.status.commit = CommitStatus::Pending;
        vec![AppEffect::ApplyAndCommit {
            repository_id,
            workspace: effect_workspace,
            git: workspace.git.clone(),
            operation: Box::new(operation),
            intent,
        }]
    }
}

#[derive(Clone)]
struct VisibleTreeEntry {
    path: std::path::PathBuf,
    kind: TreeEntryKind,
    enabled: bool,
    parent: Option<std::path::PathBuf>,
}

fn visible_tree(
    entries: &[TreeEntry],
    expanded: &std::collections::BTreeSet<std::path::PathBuf>,
) -> Vec<VisibleTreeEntry> {
    fn collect(
        output: &mut Vec<VisibleTreeEntry>,
        entries: &[TreeEntry],
        expanded: &std::collections::BTreeSet<std::path::PathBuf>,
        parent: Option<&std::path::Path>,
    ) {
        for entry in entries {
            output.push(VisibleTreeEntry {
                path: entry.path().to_path_buf(),
                kind: entry.kind(),
                enabled: entry.is_enabled(),
                parent: parent.map(std::path::Path::to_path_buf),
            });
            if entry.kind() == TreeEntryKind::Directory && expanded.contains(entry.path()) {
                collect(output, entry.children(), expanded, Some(entry.path()));
            }
        }
    }

    let mut output = Vec::new();
    collect(&mut output, entries, expanded, None);
    output
}

fn select_tree_path(workspace: &mut WorkspaceState, path: &std::path::Path) {
    let mut parent = path.parent();
    while let Some(directory) = parent.filter(|directory| !directory.as_os_str().is_empty()) {
        workspace.expanded.insert(directory.to_path_buf());
        parent = directory.parent();
    }
    let entries = visible_tree(&workspace.tree, &workspace.expanded);
    workspace.tree_selection = entries
        .iter()
        .position(|entry| entry.path == path)
        .or_else(|| (!entries.is_empty()).then_some(entries.len() - 1));
}

fn clamp_tree_selection(workspace: &mut WorkspaceState) {
    let entry_count = visible_tree(&workspace.tree, &workspace.expanded).len();
    workspace.tree_selection = if entry_count == 0 {
        None
    } else {
        Some(workspace.tree_selection.unwrap_or(0).min(entry_count - 1))
    };
}

fn rebase_path(
    path: &std::path::Path,
    from: &std::path::Path,
    to: &std::path::Path,
) -> Option<std::path::PathBuf> {
    if path == from {
        return Some(to.to_path_buf());
    }
    path.strip_prefix(from)
        .ok()
        .filter(|suffix| !suffix.as_os_str().is_empty())
        .map(|suffix| to.join(suffix))
}

fn mutation_reconciles_editor(workspace: &WorkspaceState, action: &FileMutationAction) -> bool {
    match action {
        FileMutationAction::CreateFile { .. } => workspace.editor.is_some(),
        FileMutationAction::CreateFolder { .. } => false,
        FileMutationAction::Rename { from, .. } | FileMutationAction::Move { from, .. } => {
            workspace
                .current_note
                .as_deref()
                .is_some_and(|note| note.starts_with(from))
        }
        FileMutationAction::Delete { path } => workspace
            .current_note
            .as_deref()
            .is_some_and(|note| note.starts_with(path)),
    }
}

fn is_editor_command_event(event: &AppEvent) -> bool {
    matches!(
        event,
        AppEvent::Action(AppAction::Editor(_))
            | AppEvent::Action(AppAction::Global(
                GlobalAction::Undo
                    | GlobalAction::Redo
                    | GlobalAction::Copy
                    | GlobalAction::Cut
                    | GlobalAction::Paste
                    | GlobalAction::SelectAll
            ))
            | AppEvent::ClipboardRead(Ok(_))
    )
}
