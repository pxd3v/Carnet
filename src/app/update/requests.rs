use uuid::Uuid;

use crate::{
    app::{
        App, AppEffect, DefaultChoiceState, Focus, NoteLoadPurpose, PendingRequest,
        RepositoryAvailability, RequestId, RuntimeError, RuntimeOperation, Screen, WorkspaceOrigin,
        WorkspaceState, directory_entries,
    },
    editor::Editor,
    git::GitRepo,
    workspace::{LoadedNote, TreeEntry, Workspace},
};

impl App {
    pub(super) fn handle_runtime_failed(
        &mut self,
        request_id: RequestId,
        repository_id: Uuid,
        operation: RuntimeOperation,
        error: RuntimeError,
    ) -> Vec<AppEffect> {
        let current = match operation {
            RuntimeOperation::OpenWorkspace => matches!(
                self.pending_request,
                Some(PendingRequest::OpenWorkspace {
                    request_id: pending_request,
                    repository_id: pending_repository,
                    ..
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
            RuntimeOperation::Mutation | RuntimeOperation::Push | RuntimeOperation::RefreshTree => {
                false
            }
        };
        if !current {
            return Vec::new();
        }
        self.pending_request = None;
        if operation == RuntimeOperation::OpenWorkspace
            && let Some(availability) = self
                .home
                .repositories
                .iter()
                .position(|repository| repository.id == repository_id)
                .and_then(|index| self.home.repository_availability.get_mut(index))
        {
            *availability = RepositoryAvailability::MissingOrInvalid;
        }
        self.record_request_failure(request_id, repository_id, operation, error.to_string());
        Vec::new()
    }

    pub(super) fn handle_note_loaded(
        &mut self,
        request_id: RequestId,
        repository_id: Uuid,
        note: LoadedNote,
    ) -> Vec<AppEffect> {
        let purpose = match self.pending_request.as_ref() {
            Some(PendingRequest::LoadNote {
                request_id: pending_request,
                repository_id: pending_repository,
                path,
                purpose,
            }) if *pending_request == request_id
                && *pending_repository == repository_id
                && path.as_path() == note.path().relative() =>
            {
                *purpose
            }
            _ => return Vec::new(),
        };
        if purpose == NoteLoadPurpose::Preview {
            let selected_matches = matches!(
                &self.screen,
                Screen::Workspace(workspace)
                    if workspace.tree_selection.and_then(|selected| {
                        directory_entries(&workspace.tree, &workspace.browser_directory)
                            .get(selected)
                    }).is_some_and(|entry| entry.path() == note.path().relative())
            );
            if !selected_matches {
                self.pending_request = None;
                return Vec::new();
            }
        }
        let editor_instance_id = self.next_editor_instance_id();
        let Screen::Workspace(workspace) = &mut self.screen else {
            return Vec::new();
        };
        if workspace.repository.id != repository_id {
            return Vec::new();
        }
        workspace.current_note = Some(note.path().relative().to_path_buf());
        workspace.editor = Some(Editor::from_loaded(note));
        workspace.editor_instance_id = Some(editor_instance_id);
        workspace.editor_revision = 0;
        if purpose == NoteLoadPurpose::Edit {
            workspace.focus = Focus::Editor;
            if self.sidebar.overlay_intent {
                self.sidebar.visible = false;
            }
        }
        self.pending_clipboard_read = None;
        self.pending_request = None;
        self.clear_runtime_failures(repository_id, RuntimeOperation::LoadNote);
        Vec::new()
    }

    pub(super) fn handle_workspace_opened(
        &mut self,
        request_id: RequestId,
        repository_id: Uuid,
        workspace: Workspace,
        git: GitRepo,
        tree: Vec<TreeEntry>,
        note: Option<LoadedNote>,
    ) -> Vec<AppEffect> {
        let current_registration = self
            .home
            .repositories
            .iter()
            .find(|repository| **repository == *workspace.repo());
        let request_matches = matches!(
            &self.pending_request,
            Some(PendingRequest::OpenWorkspace {
                request_id: pending_request,
                repository_id: pending_repository,
                repository: pending_registration,
            }) if *pending_request == request_id
                && *pending_repository == repository_id
                && pending_registration == workspace.repo()
        );
        if workspace.repo().id != repository_id
            || current_registration.is_none()
            || !request_matches
        {
            return Vec::new();
        }
        self.pending_request = None;
        if let Some(availability) = self
            .home
            .repositories
            .iter()
            .position(|repository| repository.id == repository_id)
            .and_then(|index| self.home.repository_availability.get_mut(index))
        {
            *availability = RepositoryAvailability::Available;
        }
        let current_note = note
            .as_ref()
            .map(|note| note.path().relative().to_path_buf());
        let editor_instance_id = note.as_ref().map(|_| self.next_editor_instance_id());
        let opened_origin = WorkspaceOrigin {
            repository_id,
            repository_root: workspace.root().to_path_buf(),
        };
        let repository = workspace.repo().clone();
        let browser_directory = current_note
            .as_deref()
            .and_then(std::path::Path::parent)
            .unwrap_or_else(|| std::path::Path::new(""))
            .to_path_buf();
        let browser_entries = directory_entries(&tree, &browser_directory);
        let tree_selection = current_note
            .as_ref()
            .and_then(|path| {
                browser_entries
                    .iter()
                    .position(|entry| entry.path() == path)
            })
            .or_else(|| (!browser_entries.is_empty()).then_some(0));
        let focus = if note.is_some() {
            Focus::Editor
        } else {
            Focus::Tree
        };
        let preview_initial_selection = current_note.is_none();
        self.screen = Screen::Workspace(Box::new(WorkspaceState {
            repository,
            workspace,
            git,
            tree,
            current_note,
            editor: note.map(Editor::from_loaded),
            editor_instance_id,
            editor_revision: 0,
            focus,
            browser_directory,
            tree_selection,
        }));
        self.invalidate_workspace_bound_state(&opened_origin);
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
        if preview_initial_selection {
            self.reconcile_browser_preview()
        } else {
            Vec::new()
        }
    }
}
