use std::path::{Path, PathBuf};

use uuid::Uuid;

use super::effect::RuntimeOperation;
use crate::{
    catalog::RepoEntry,
    editor::Editor,
    git::{CommitIntent, GitRepo},
    workspace::{TreeEntry, TreeEntryKind, Workspace},
};

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct RequestId(pub(crate) u64);

impl RequestId {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub fn get(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct MutationId(pub(crate) u64);

impl MutationId {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub fn get(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct PushId(pub(crate) u64);

impl PushId {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub fn get(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ClipboardRequestId(pub(crate) u64);

impl ClipboardRequestId {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub fn get(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct EditorInstanceId(pub(crate) u64);

impl EditorInstanceId {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub fn get(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EditorOrigin {
    pub repository_id: Uuid,
    pub repository_root: PathBuf,
    pub note_path: PathBuf,
    pub instance_id: EditorInstanceId,
    pub revision: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingClipboardRead {
    pub request_id: ClipboardRequestId,
    pub origin: EditorOrigin,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PendingRequest {
    OpenWorkspace {
        request_id: RequestId,
        repository_id: Uuid,
        repository: RepoEntry,
    },
    LoadNote {
        request_id: RequestId,
        repository_id: Uuid,
        path: PathBuf,
        purpose: NoteLoadPurpose,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NoteLoadPurpose {
    Preview,
    Edit,
}

impl PendingRequest {
    pub fn request_id(&self) -> RequestId {
        match self {
            Self::OpenWorkspace { request_id, .. } | Self::LoadNote { request_id, .. } => {
                *request_id
            }
        }
    }

    pub fn path(&self) -> Option<&std::path::Path> {
        match self {
            Self::OpenWorkspace { .. } => None,
            Self::LoadNote { path, .. } => Some(path),
        }
    }
}

pub enum Screen {
    Home,
    Workspace(Box<WorkspaceState>),
}

pub struct WorkspaceState {
    pub repository: RepoEntry,
    pub workspace: Workspace,
    pub git: GitRepo,
    pub tree: Vec<TreeEntry>,
    pub current_note: Option<PathBuf>,
    pub editor: Option<Editor>,
    pub editor_instance_id: Option<EditorInstanceId>,
    pub editor_revision: u64,
    pub focus: Focus,
    pub browser_directory: PathBuf,
    pub tree_selection: Option<usize>,
}

pub(crate) fn directory_entries<'a>(entries: &'a [TreeEntry], directory: &Path) -> &'a [TreeEntry] {
    if directory.as_os_str().is_empty() {
        return entries;
    }
    fn find<'a>(entries: &'a [TreeEntry], directory: &Path) -> Option<&'a [TreeEntry]> {
        for entry in entries {
            if entry.kind() == TreeEntryKind::Directory {
                if entry.path() == directory {
                    return Some(entry.children());
                }
                if let Some(children) = find(entry.children(), directory) {
                    return Some(children);
                }
            }
        }
        None
    }
    find(entries, directory).unwrap_or(&[])
}

impl WorkspaceState {
    pub fn matching_text_paths(&self, query: &str) -> Vec<PathBuf> {
        fn collect(output: &mut Vec<PathBuf>, entries: &[TreeEntry]) {
            for entry in entries {
                if entry.kind() == TreeEntryKind::Directory {
                    collect(output, entry.children());
                } else if entry.kind() == TreeEntryKind::File && entry.is_enabled() {
                    output.push(entry.path().to_path_buf());
                }
            }
        }

        let mut paths = Vec::new();
        collect(&mut paths, &self.tree);
        let query = query.to_lowercase();
        paths.retain(|path| path.to_string_lossy().to_lowercase().contains(&query));
        paths
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Focus {
    Editor,
    Tree,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DefaultChoiceState {
    NotNeeded,
    AwaitingSelection,
    ResumingPendingNote { repository_id: Uuid, note: PathBuf },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HomeState {
    pub repositories: Vec<RepoEntry>,
    pub repository_availability: Vec<RepositoryAvailability>,
    pub selected: Option<usize>,
    pub default_repository: Option<Uuid>,
    pub pending_note: Option<PathBuf>,
    pub default_choice: DefaultChoiceState,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CatalogSnapshot {
    pub repositories: Vec<RepoEntry>,
    pub default_repository: Option<Uuid>,
    pub selected_repository: Option<Uuid>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingDefaultIntent {
    pub repository: RepoEntry,
    pub note: Option<PathBuf>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PendingCatalogOperation {
    Create,
    Register,
    Rename { repository_id: Uuid },
    SetDefault(PendingDefaultIntent),
    Unregister { repository_id: Uuid },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RepositoryAvailability {
    Available,
    MissingOrInvalid,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum OverlayState {
    #[default]
    None,
    Search {
        query: String,
    },
    QuickOpen {
        query: String,
        selected: Option<usize>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SidebarState {
    pub visible: bool,
    pub overlay_intent: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PendingMutationKind {
    Save { overwrite: bool },
    RetryCommit,
    File(FileActionKind),
    Delete,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingMutation {
    pub mutation_id: MutationId,
    pub repository_id: Uuid,
    pub repository_root: PathBuf,
    pub kind: PendingMutationKind,
    pub intent: CommitIntent,
    pub save: Option<PendingSave>,
    pub reconciles_editor: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingPush {
    pub push_id: PushId,
    pub repository_id: Uuid,
    pub repository_root: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingSave {
    pub generation: u64,
    pub snapshot: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NavigationAction {
    Home,
    Repository {
        repository: RepoEntry,
        note: Option<PathBuf>,
    },
    Note(PathBuf),
    Quit,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FileMutationAction {
    CreateFile { path: PathBuf },
    CreateFolder { path: PathBuf },
    Rename { from: PathBuf, to: PathBuf },
    Move { from: PathBuf, to: PathBuf },
    Delete { path: PathBuf },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkspaceOrigin {
    pub repository_id: Uuid,
    pub repository_root: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingFileMutation {
    pub origin: WorkspaceOrigin,
    pub action: FileMutationAction,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PendingIntent {
    Navigation(NavigationAction),
    Mutation(PendingFileMutation),
    BrowseFiles,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Dialog {
    DirtyNavigation,
    ExternalConflict(ExternalConflict),
    SavedCommitFailed {
        message: String,
    },
    Failure {
        kind: FailureKind,
        message: String,
    },
    FileAction {
        origin: WorkspaceOrigin,
        kind: FileActionKind,
        target: Option<PathBuf>,
    },
    ConfirmDelete {
        origin: WorkspaceOrigin,
        path: PathBuf,
    },
    RepositoryForm {
        kind: RepositoryActionKind,
        repository_id: Option<Uuid>,
    },
    ConfirmSetDefault {
        repository_id: Uuid,
        name: String,
    },
    ConfirmUnregister {
        repository_id: Uuid,
        name: String,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RepositoryActionKind {
    Create,
    Register,
    Rename,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum RepositoryFormField {
    #[default]
    Name,
    Path,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RepositoryFormState {
    pub name: String,
    pub path: String,
    pub active_field: RepositoryFormField,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExternalConflict {
    Modified { path: PathBuf },
    Deleted { path: PathBuf },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FileActionKind {
    NewFile,
    NewFolder,
    Rename,
    Move,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum CommitStatus {
    #[default]
    Idle,
    Pending,
    Committed {
        revision: String,
    },
    NoChanges,
    SavedCommitFailed {
        message: String,
    },
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum PushStatus {
    #[default]
    Idle,
    Pushing,
    Pushed,
    UpToDate,
    Failed {
        message: String,
    },
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct StatusState {
    pub commit: CommitStatus,
    pub push: PushStatus,
    pub message: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AppExitStatus {
    Success,
    Failure,
}

impl AppExitStatus {
    pub fn code(self) -> u8 {
        match self {
            Self::Success => 0,
            Self::Failure => 1,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct QuitState {
    pub requested: bool,
    pub final_status: Option<AppExitStatus>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FailureKind {
    Runtime,
    Write,
    Git,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UnresolvedFailure {
    pub kind: FailureKind,
    pub message: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeFailure {
    pub request_id: Option<RequestId>,
    pub repository_id: Uuid,
    pub operation: RuntimeOperation,
    pub message: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FailureState {
    pub runtime: Vec<RuntimeFailure>,
    pub runtime_driver: Option<UnresolvedFailure>,
    pub write: Option<UnresolvedFailure>,
    pub git: Option<UnresolvedFailure>,
    pub push: Option<UnresolvedFailure>,
    pub clipboard: Option<UnresolvedFailure>,
    pub catalog: Option<UnresolvedFailure>,
}

impl FailureState {
    pub fn is_empty(&self) -> bool {
        self.runtime.is_empty()
            && self.runtime_driver.is_none()
            && self.write.is_none()
            && self.git.is_none()
            && self.push.is_none()
            && self.clipboard.is_none()
            && self.catalog.is_none()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SavedCommitFailure {
    pub repository_id: Uuid,
    pub intent: CommitIntent,
    pub message: String,
}

pub struct App {
    pub screen: Screen,
    pub home: HomeState,
    pub sidebar: SidebarState,
    pub overlay: OverlayState,
    pub pending_mutation: Option<PendingMutation>,
    pub pending_push: Option<PendingPush>,
    pub pending_intent: Option<PendingIntent>,
    pub pending_request: Option<PendingRequest>,
    pub pending_catalog: Option<PendingCatalogOperation>,
    pub pending_clipboard_read: Option<PendingClipboardRead>,
    pub dialog: Option<Dialog>,
    pub dialog_input: String,
    pub repository_form: RepositoryFormState,
    pub status: StatusState,
    pub quit: QuitState,
    pub failures: FailureState,
    pub saved_commit_failure: Option<SavedCommitFailure>,
    pub(crate) next_request_id: u64,
    pub(crate) next_mutation_id: u64,
    pub(crate) next_push_id: u64,
    pub(crate) next_save_generation: u64,
    pub(crate) next_clipboard_request_id: u64,
    pub(crate) next_editor_instance_id: u64,
}

impl App {
    pub fn home(
        repositories: Vec<RepoEntry>,
        default_repository: Option<Uuid>,
        pending_note: Option<PathBuf>,
    ) -> Self {
        let repository_availability = vec![RepositoryAvailability::Available; repositories.len()];
        let selected = default_repository
            .and_then(|id| {
                repositories
                    .iter()
                    .position(|repository| repository.id == id)
            })
            .or((!repositories.is_empty()).then_some(0));
        Self {
            screen: Screen::Home,
            sidebar: SidebarState {
                visible: true,
                overlay_intent: false,
            },
            overlay: OverlayState::None,
            pending_mutation: None,
            pending_push: None,
            pending_intent: None,
            pending_request: None,
            pending_catalog: None,
            pending_clipboard_read: None,
            dialog: None,
            dialog_input: String::new(),
            repository_form: RepositoryFormState::default(),
            status: StatusState::default(),
            quit: QuitState::default(),
            failures: FailureState::default(),
            saved_commit_failure: None,
            next_request_id: 1,
            next_mutation_id: 1,
            next_push_id: 1,
            next_save_generation: 1,
            next_clipboard_request_id: 1,
            next_editor_instance_id: 1,
            home: HomeState {
                repositories,
                repository_availability,
                selected,
                default_repository,
                default_choice: if pending_note.is_some() && default_repository.is_none() {
                    DefaultChoiceState::AwaitingSelection
                } else {
                    DefaultChoiceState::NotNeeded
                },
                pending_note,
            },
        }
    }
}
