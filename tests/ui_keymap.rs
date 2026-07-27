use std::{fs, path::PathBuf};

use carnet::{
    app::{
        App, AppAction, AppEvent, ConflictChoice, Dialog, DirtyChoice, Focus, GlobalAction,
        HomeAction, OverlayState, Screen, TreeAction,
    },
    catalog::RepoEntry,
    git::GitRepo,
    ui::{COMFORTABLE_WIDTH, map_key, workspace_geometry},
    workspace::Workspace,
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use tempfile::{TempDir, tempdir};
use uuid::Uuid;

#[test]
fn narrow_geometry_preserves_the_full_editor_and_overlays_the_tree() {
    let area = Rect::new(0, 0, COMFORTABLE_WIDTH - 1, 20);

    let geometry = workspace_geometry(area, true);

    assert_eq!(geometry.editor, area);
    assert!(geometry.tree_is_overlay);
    assert_eq!(geometry.tree.unwrap().x, area.x);
    assert!(geometry.tree.unwrap().width < geometry.editor.width);

    let wide = workspace_geometry(Rect::new(0, 0, COMFORTABLE_WIDTH, 20), true);
    assert!(!wide.tree_is_overlay);
    assert_eq!(wide.tree.unwrap().width, 30);
    assert_eq!(wide.editor.x, 30);
}

#[test]
fn ctrl_shortcuts_map_to_portable_global_actions() {
    let app = App::home(Vec::new(), None, None);
    let cases = [
        ('s', KeyModifiers::CONTROL, GlobalAction::Save),
        ('f', KeyModifiers::CONTROL, GlobalAction::Find),
        ('p', KeyModifiers::CONTROL, GlobalAction::QuickOpen),
        ('b', KeyModifiers::CONTROL, GlobalAction::ToggleSidebar),
        ('z', KeyModifiers::CONTROL, GlobalAction::Undo),
        ('y', KeyModifiers::CONTROL, GlobalAction::Redo),
        (
            'Z',
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
            GlobalAction::Redo,
        ),
        ('c', KeyModifiers::CONTROL, GlobalAction::Copy),
        ('x', KeyModifiers::CONTROL, GlobalAction::Cut),
        ('v', KeyModifiers::CONTROL, GlobalAction::Paste),
        ('a', KeyModifiers::CONTROL, GlobalAction::SelectAll),
        ('q', KeyModifiers::CONTROL, GlobalAction::Quit),
    ];

    for (character, modifiers, expected) in cases {
        assert_eq!(
            mapped_action(&app, KeyEvent::new(KeyCode::Char(character), modifiers)),
            AppAction::Global(expected),
            "shortcut Ctrl+{character}"
        );
    }
}

#[test]
fn home_and_tree_keys_map_only_to_their_pure_actions() {
    let home = App::home(Vec::new(), None, None);
    assert_eq!(
        mapped_action(&home, KeyEvent::from(KeyCode::Up)),
        AppAction::Home(HomeAction::Up)
    );
    assert_eq!(
        mapped_action(&home, KeyEvent::from(KeyCode::Down)),
        AppAction::Home(HomeAction::Down)
    );
    assert_eq!(
        mapped_action(&home, KeyEvent::from(KeyCode::Enter)),
        AppAction::Home(HomeAction::OpenSelected)
    );

    let (_sandbox, mut workspace) = workspace_app();
    let Screen::Workspace(state) = &mut workspace.screen else {
        panic!("workspace fixture did not open")
    };
    state.focus = Focus::Tree;
    let cases = [
        (KeyEvent::from(KeyCode::Up), TreeAction::Up),
        (KeyEvent::from(KeyCode::Down), TreeAction::Down),
        (KeyEvent::from(KeyCode::Left), TreeAction::Left),
        (KeyEvent::from(KeyCode::Right), TreeAction::Right),
        (KeyEvent::from(KeyCode::Enter), TreeAction::Open),
        (
            KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE),
            TreeAction::NewFile,
        ),
        (
            KeyEvent::new(KeyCode::Char('N'), KeyModifiers::SHIFT),
            TreeAction::NewFolder,
        ),
        (
            KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE),
            TreeAction::Rename,
        ),
        (
            KeyEvent::new(KeyCode::Char('m'), KeyModifiers::NONE),
            TreeAction::Move,
        ),
        (KeyEvent::from(KeyCode::Delete), TreeAction::Delete),
        (KeyEvent::from(KeyCode::Esc), TreeAction::Escape),
    ];
    for (key, expected) in cases {
        assert_eq!(mapped_action(&workspace, key), AppAction::Tree(expected));
    }
}

#[test]
fn modal_choices_and_search_input_map_to_explicit_app_events() {
    let mut app = App::home(Vec::new(), None, None);
    app.dialog = Some(Dialog::DirtyNavigation);
    assert!(matches!(
        map_key(&app, KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE)),
        Some(AppEvent::DirtyChoice(DirtyChoice::Save))
    ));
    assert!(matches!(
        map_key(&app, KeyEvent::from(KeyCode::Esc)),
        Some(AppEvent::DirtyChoice(DirtyChoice::Cancel))
    ));

    app.dialog = Some(Dialog::ExternalConflict(
        carnet::app::ExternalConflict::Modified {
            path: PathBuf::from("note.md"),
        },
    ));
    assert!(matches!(
        map_key(&app, KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE)),
        Some(AppEvent::ConflictChoice(ConflictChoice::Overwrite))
    ));

    app.dialog = None;
    app.overlay = OverlayState::Search { query: "ca".into() };
    assert_eq!(
        mapped_action(&app, KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE)),
        AppAction::SetOverlayQuery("cat".into())
    );
    assert_eq!(
        mapped_action(&app, KeyEvent::from(KeyCode::Backspace)),
        AppAction::SetOverlayQuery("c".into())
    );

    let event = map_key(&app, KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE)).unwrap();
    app.update(event);
    assert_eq!(
        app.overlay,
        OverlayState::Search {
            query: "cat".into()
        }
    );
}

#[test]
fn file_dialog_and_quick_open_keys_preserve_input_in_app_actions() {
    let (_sandbox, mut app) = workspace_app();
    let Screen::Workspace(workspace) = &mut app.screen else {
        panic!("workspace fixture did not open")
    };
    workspace.focus = Focus::Tree;
    app.update(AppEvent::Action(AppAction::Tree(TreeAction::NewFile)));

    assert_eq!(
        mapped_action(&app, KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE)),
        AppAction::SetDialogInput("n".into())
    );
    app.update(AppEvent::Action(AppAction::SetDialogInput("new.md".into())));
    assert_eq!(app.dialog_input, "new.md");
    assert_eq!(
        mapped_action(&app, KeyEvent::from(KeyCode::Enter)),
        AppAction::SubmitFileAction(PathBuf::from("new.md"))
    );

    app.dialog = None;
    app.overlay = OverlayState::QuickOpen {
        query: "note".into(),
        selected: Some(0),
    };
    assert_eq!(
        mapped_action(&app, KeyEvent::from(KeyCode::Down)),
        AppAction::MoveOverlaySelection(1)
    );
    assert_eq!(
        mapped_action(&app, KeyEvent::from(KeyCode::Enter)),
        AppAction::SubmitOverlay
    );
}

#[test]
fn quick_open_selects_and_opens_the_first_matching_text_file() {
    let (_sandbox, mut app) = workspace_app();

    app.update(AppEvent::Action(AppAction::Global(GlobalAction::QuickOpen)));

    assert_eq!(
        app.overlay,
        OverlayState::QuickOpen {
            query: String::new(),
            selected: Some(0),
        }
    );
    let event = map_key(&app, KeyEvent::from(KeyCode::Enter)).unwrap();
    let effects = app.update(event);
    assert!(matches!(
        effects.as_slice(),
        [carnet::app::AppEffect::LoadNote { path, .. }] if path == &PathBuf::from("note.md")
    ));
}

fn mapped_action(app: &App, key: KeyEvent) -> AppAction {
    match map_key(app, key) {
        Some(AppEvent::Action(action)) => action,
        _ => panic!("key did not map to an app action"),
    }
}

fn workspace_app() -> (TempDir, App) {
    let sandbox = tempdir().unwrap();
    let root = fs::canonicalize(sandbox.path()).unwrap();
    fs::write(root.join("note.md"), "note").unwrap();
    let repository = RepoEntry {
        id: Uuid::from_u128(7),
        name: "notes".into(),
        path: root,
    };
    let git = GitRepo::initialize(&repository.path).unwrap();
    let workspace = Workspace::open(repository.clone()).unwrap();
    let tree = workspace.tree().unwrap();
    let note = workspace
        .load_note(
            &workspace
                .resolve_note(PathBuf::from("note.md").as_path())
                .unwrap(),
        )
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
