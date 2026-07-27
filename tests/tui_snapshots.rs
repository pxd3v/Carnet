use std::{fs, path::PathBuf};

use carnet::{
    app::{App, AppAction, AppEvent, CommitStatus, Dialog, ExternalConflict, HomeAction, Screen},
    catalog::RepoEntry,
    editor::{EditorCommand, Motion},
    git::GitRepo,
    ui::render,
    workspace::Workspace,
};
use ratatui::{Terminal, backend::TestBackend};
use tempfile::{TempDir, tempdir};
use uuid::Uuid;

#[test]
fn repository_home_is_a_discoverable_first_run_surface() {
    let app = App::home(Vec::new(), None, None);
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).unwrap();

    terminal.draw(|frame| render(frame, &app)).unwrap();

    insta::assert_snapshot!(terminal.backend());
}

#[test]
fn wide_workspace_keeps_tree_and_editor_visible() {
    let (_sandbox, mut app) = workspace_app("notes/welcome.md", "# Welcome\n\nWrite freely.\n");
    let Screen::Workspace(workspace) = &mut app.screen else {
        panic!("workspace fixture did not open")
    };
    workspace.expanded.insert(PathBuf::from("notes"));
    let editor = workspace.editor.as_mut().unwrap();
    editor.apply(EditorCommand::Move {
        motion: Motion::DocumentEnd,
        extend_selection: false,
    });
    editor.apply(EditorCommand::Insert("Local edit".into()));
    let backend = TestBackend::new(110, 30);
    let mut terminal = Terminal::new(backend).unwrap();

    terminal.draw(|frame| render(frame, &app)).unwrap();

    insta::assert_snapshot!(terminal.backend());
}

#[test]
fn narrow_workspace_floats_the_tree_over_a_full_editor() {
    let (_sandbox, mut app) = workspace_app("notes/welcome.md", "# Welcome\n\nWrite freely.\n");
    let Screen::Workspace(workspace) = &mut app.screen else {
        panic!("workspace fixture did not open")
    };
    workspace.expanded.insert(PathBuf::from("notes"));
    workspace.focus = carnet::app::Focus::Tree;
    let backend = TestBackend::new(72, 22);
    let mut terminal = Terminal::new(backend).unwrap();

    terminal.draw(|frame| render(frame, &app)).unwrap();

    insta::assert_snapshot!(terminal.backend());
}

#[test]
fn dirty_navigation_prompt_exposes_save_discard_and_cancel() {
    let (_sandbox, mut app) = workspace_app("notes/welcome.md", "# Welcome\n");
    app.sidebar.visible = false;
    let Screen::Workspace(workspace) = &mut app.screen else {
        panic!("workspace fixture did not open")
    };
    workspace
        .editor
        .as_mut()
        .unwrap()
        .apply(EditorCommand::Insert("Draft ".into()));
    app.dialog = Some(Dialog::DirtyNavigation);
    let backend = TestBackend::new(90, 24);
    let mut terminal = Terminal::new(backend).unwrap();

    terminal.draw(|frame| render(frame, &app)).unwrap();

    insta::assert_snapshot!(terminal.backend());
}

#[test]
fn external_conflict_prompt_exposes_reload_overwrite_and_cancel() {
    let (_sandbox, mut app) = workspace_app("notes/welcome.md", "# Welcome\n");
    app.sidebar.visible = false;
    app.dialog = Some(Dialog::ExternalConflict(ExternalConflict::Modified {
        path: PathBuf::from("notes/welcome.md"),
    }));
    let backend = TestBackend::new(90, 24);
    let mut terminal = Terminal::new(backend).unwrap();

    terminal.draw(|frame| render(frame, &app)).unwrap();

    insta::assert_snapshot!(terminal.backend());
}

#[test]
fn git_failure_distinguishes_saved_from_committed_and_offers_retry() {
    let (_sandbox, mut app) = workspace_app("notes/welcome.md", "# Welcome\n");
    app.sidebar.visible = false;
    let message = "Git identity is not configured".to_owned();
    app.status.commit = CommitStatus::SavedCommitFailed {
        message: message.clone(),
    };
    app.status.message = Some(message.clone());
    app.dialog = Some(Dialog::SavedCommitFailed { message });
    let backend = TestBackend::new(90, 24);
    let mut terminal = Terminal::new(backend).unwrap();

    terminal.draw(|frame| render(frame, &app)).unwrap();

    insta::assert_snapshot!(terminal.backend());
}

fn workspace_app(note_path: &str, contents: &str) -> (TempDir, App) {
    let sandbox = tempdir().unwrap();
    let root = fs::canonicalize(sandbox.path()).unwrap();
    fs::create_dir_all(root.join("notes")).unwrap();
    fs::write(root.join(note_path), contents).unwrap();
    fs::write(root.join("notes/archive.bin"), b"binary\0content").unwrap();
    fs::write(root.join("README.txt"), "plain text\n").unwrap();
    let repository = RepoEntry {
        id: Uuid::from_u128(42),
        name: "field-notes".into(),
        path: root,
    };
    let git = GitRepo::initialize(&repository.path).unwrap();
    let workspace = Workspace::open(repository.clone()).unwrap();
    let tree = workspace.tree().unwrap();
    let note = workspace
        .load_note(&workspace.resolve_note(note_path.as_ref()).unwrap())
        .unwrap();
    let mut app = App::home(vec![repository.clone()], Some(repository.id), None);
    app.update(AppEvent::Action(AppAction::Home(HomeAction::OpenSelected)));
    let request_id = app.pending_request.as_ref().unwrap().request_id();
    app.update(AppEvent::WorkspaceOpened {
        request_id,
        repository_id: repository.id,
        workspace,
        git,
        tree,
        note: Some(note),
    });
    (sandbox, app)
}
