use std::{collections::BTreeSet, path::PathBuf};

use uuid::Uuid;

use crate::{
    catalog::RepoEntry,
    editor::Editor,
    git::{CommitIntent, GitRepo},
    workspace::{TreeEntry, Workspace},
};

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
    pub focus: Focus,
    pub tree_selection: Option<usize>,
    pub expanded: BTreeSet<PathBuf>,
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
    pub selected: Option<usize>,
    pub default_repository: Option<Uuid>,
    pub pending_note: Option<PathBuf>,
    pub default_choice: DefaultChoiceState,
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
    pub repository_id: Uuid,
    pub kind: PendingMutationKind,
    pub intent: CommitIntent,
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
        kind: FileActionKind,
        target: Option<PathBuf>,
    },
    ConfirmDelete {
        path: PathBuf,
    },
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
pub struct StatusState {
    pub commit: CommitStatus,
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
    pub pending_navigation: Option<NavigationAction>,
    pub pending_load: Option<PathBuf>,
    pub dialog: Option<Dialog>,
    pub status: StatusState,
    pub quit: QuitState,
    pub failure: Option<UnresolvedFailure>,
    pub saved_commit_failure: Option<SavedCommitFailure>,
}

impl App {
    pub fn home(
        repositories: Vec<RepoEntry>,
        default_repository: Option<Uuid>,
        pending_note: Option<PathBuf>,
    ) -> Self {
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
            pending_navigation: None,
            pending_load: None,
            dialog: None,
            status: StatusState::default(),
            quit: QuitState::default(),
            failure: None,
            saved_commit_failure: None,
            home: HomeState {
                repositories,
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
