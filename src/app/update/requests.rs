use uuid::Uuid;

use crate::{
    app::{
        App, AppEffect, DefaultChoiceState, Focus, PendingRequest, RepositoryAvailability,
        RequestId, RuntimeError, RuntimeOperation, Screen, WorkspaceOrigin, WorkspaceState,
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
            RuntimeOperation::Mutation | RuntimeOperation::RefreshTree => false,
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
        let tree_selection = (!tree.is_empty()).then_some(0);
        let focus = if note.is_some() {
            Focus::Editor
        } else {
            Focus::Tree
        };
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
            tree_selection,
            expanded: Default::default(),
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
        Vec::new()
    }
}
