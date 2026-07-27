use std::{fs, path::PathBuf};

use carnet::{
    app::{
        App, AppAction, AppEvent, ConflictChoice, Dialog, DirtyChoice, FileActionKind, Focus,
        GlobalAction, HomeAction, OverlayState, RepositoryActionKind, RepositoryFormField, Screen,
        TreeAction,
    },
    catalog::RepoEntry,
    editor::{EditorCommand, Motion},
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
fn editor_escape_requests_a_safe_return_to_files() {
    let (_sandbox, app) = workspace_app();

    assert_eq!(
        mapped_action(&app, KeyEvent::from(KeyCode::Esc)),
        AppAction::BrowseFiles
    );
}

#[test]
fn editor_modifier_arrows_map_to_word_line_and_document_motion() {
    let (_sandbox, app) = workspace_app();
    let cases = [
        (KeyCode::Left, KeyModifiers::ALT, Motion::WordLeft, false),
        (KeyCode::Right, KeyModifiers::ALT, Motion::WordRight, false),
        (KeyCode::Left, KeyModifiers::SUPER, Motion::LineStart, false),
        (KeyCode::Right, KeyModifiers::SUPER, Motion::LineEnd, false),
        (
            KeyCode::Up,
            KeyModifiers::SUPER,
            Motion::DocumentStart,
            false,
        ),
        (
            KeyCode::Down,
            KeyModifiers::SUPER | KeyModifiers::SHIFT,
            Motion::DocumentEnd,
            true,
        ),
    ];

    for (code, modifiers, motion, extend_selection) in cases {
        assert_eq!(
            mapped_action(&app, KeyEvent::new(code, modifiers)),
            AppAction::Editor(EditorCommand::Move {
                motion,
                extend_selection,
            })
        );
    }
    assert_eq!(
        mapped_action(&app, KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT)),
        AppAction::Editor(EditorCommand::Newline)
    );
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

#[test]
fn ctrl_b_reaches_file_actions_and_enter_returns_to_the_loaded_editor() {
    let (_sandbox, mut app) = workspace_app();
    app.sidebar.visible = false;

    let event = map_key(
        &app,
        KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL),
    )
    .unwrap();
    app.update(event);
    let Screen::Workspace(workspace) = &app.screen else {
        panic!("workspace fixture did not open")
    };
    assert!(app.sidebar.visible);
    assert_eq!(workspace.focus, Focus::Tree);

    for (key, expected) in [
        (
            KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE),
            FileActionKind::NewFile,
        ),
        (
            KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE),
            FileActionKind::Rename,
        ),
        (
            KeyEvent::new(KeyCode::Char('m'), KeyModifiers::NONE),
            FileActionKind::Move,
        ),
    ] {
        let event = map_key(&app, key).unwrap();
        app.update(event);
        assert!(matches!(
            app.dialog,
            Some(Dialog::FileAction { kind, .. }) if kind == expected
        ));
        app.update(AppEvent::Action(AppAction::Dismiss));
    }

    let event = map_key(&app, KeyEvent::from(KeyCode::Delete)).unwrap();
    app.update(event);
    assert!(matches!(app.dialog, Some(Dialog::ConfirmDelete { .. })));
    app.update(AppEvent::Action(AppAction::Dismiss));

    let open = map_key(&app, KeyEvent::from(KeyCode::Enter)).unwrap();
    assert!(app.update(open).is_empty());
    let Screen::Workspace(workspace) = &app.screen else {
        panic!("workspace fixture did not open")
    };
    assert_eq!(workspace.focus, Focus::Editor);

    let event = map_key(
        &app,
        KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL),
    )
    .unwrap();
    app.update(event);
    let Screen::Workspace(workspace) = &app.screen else {
        panic!("workspace fixture did not open")
    };
    assert!(app.sidebar.visible);
    assert_eq!(workspace.focus, Focus::Tree);
}

#[test]
fn dialogs_and_overlays_consume_keys_that_are_not_explicit_choices() {
    let (_sandbox, mut app) = workspace_app();
    let dialogs = [
        Dialog::DirtyNavigation,
        Dialog::ExternalConflict(carnet::app::ExternalConflict::Modified {
            path: PathBuf::from("note.md"),
        }),
        Dialog::Failure {
            kind: carnet::app::FailureKind::Runtime,
            message: "failed".into(),
        },
    ];

    for dialog in dialogs {
        app.dialog = Some(dialog);
        assert!(
            map_key(
                &app,
                KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL)
            )
            .is_none()
        );
        assert!(map_key(&app, KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE)).is_none());
        assert!(map_key(&app, KeyEvent::from(KeyCode::Up)).is_none());
    }

    app.dialog = None;
    app.overlay = OverlayState::Search {
        query: String::new(),
    };
    assert!(
        map_key(
            &app,
            KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL)
        )
        .is_none()
    );
    assert!(map_key(&app, KeyEvent::from(KeyCode::Up)).is_none());

    app.overlay = OverlayState::None;
    let Screen::Workspace(workspace) = &app.screen else {
        panic!("workspace fixture did not open")
    };
    let origin = carnet::app::WorkspaceOrigin {
        repository_id: workspace.repository.id,
        repository_root: workspace.workspace.root().to_path_buf(),
    };
    let editor_before = workspace.editor.as_ref().unwrap().text();
    app.dialog = Some(Dialog::FileAction {
        origin,
        kind: FileActionKind::NewFile,
        target: None,
    });
    assert!(map_key(&app, KeyEvent::from(KeyCode::Enter)).is_none());
    assert!(
        map_key(
            &app,
            KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL)
        )
        .is_none()
    );
    let file_input = map_key(&app, KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE)).unwrap();
    app.update(file_input);
    assert_eq!(app.dialog_input, "x");
    let Screen::Workspace(workspace) = &app.screen else {
        panic!("workspace fixture did not open")
    };
    assert_eq!(workspace.editor.as_ref().unwrap().text(), editor_before);

    app.dialog = Some(Dialog::RepositoryForm {
        kind: RepositoryActionKind::Create,
        repository_id: None,
    });
    app.repository_form = Default::default();
    assert!(map_key(&app, KeyEvent::from(KeyCode::Enter)).is_none());
    assert!(
        map_key(
            &app,
            KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL)
        )
        .is_none()
    );
    let repository_input =
        map_key(&app, KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE)).unwrap();
    app.update(repository_input);
    assert_eq!(app.repository_form.name, "x");
    let Screen::Workspace(workspace) = &app.screen else {
        panic!("workspace fixture did not open")
    };
    assert_eq!(workspace.editor.as_ref().unwrap().text(), editor_before);
}

#[test]
fn disabled_home_rows_are_inert_for_open_and_default_actions() {
    let (_sandbox, workspace) = workspace_app();
    let Screen::Workspace(workspace_state) = &workspace.screen else {
        panic!("workspace fixture did not open")
    };
    let repository = workspace_state.repository.clone();
    let mut app = App::home(vec![repository.clone()], Some(repository.id), None);
    app.home.repository_availability[0] = carnet::app::RepositoryAvailability::MissingOrInvalid;

    let open = map_key(&app, KeyEvent::from(KeyCode::Enter)).unwrap();
    assert!(app.update(open).is_empty());
    assert!(app.pending_request.is_none());

    app.home.pending_note = Some(PathBuf::from("pending.md"));
    let set_default = map_key(&app, KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE)).unwrap();
    assert!(app.update(set_default).is_empty());
    assert!(app.pending_request.is_none());
}

#[test]
fn repository_create_and_register_forms_emit_typed_outer_effects() {
    let mut app = App::home(Vec::new(), None, None);

    let create = map_key(&app, KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE)).unwrap();
    app.update(create);
    assert!(matches!(
        app.dialog,
        Some(Dialog::RepositoryForm {
            kind: RepositoryActionKind::Create,
            repository_id: None,
        })
    ));
    type_modal_text(&mut app, "journal");
    app.update(map_key(&app, KeyEvent::from(KeyCode::Tab)).unwrap());
    assert_eq!(app.repository_form.active_field, RepositoryFormField::Path);
    type_modal_text(&mut app, "/tmp/journal");
    let effects = app.update(map_key(&app, KeyEvent::from(KeyCode::Enter)).unwrap());
    assert!(matches!(
        effects.as_slice(),
        [carnet::app::AppEffect::CreateRepository { name, path }]
            if name == "journal" && path == &PathBuf::from("/tmp/journal")
    ));
    app.update(AppEvent::RepositoryCatalogFailed {
        message: "create stopped for test".into(),
    });
    app.update(AppEvent::Action(AppAction::Dismiss));

    let register = map_key(&app, KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE)).unwrap();
    app.update(register);
    assert!(matches!(
        app.dialog,
        Some(Dialog::RepositoryForm {
            kind: RepositoryActionKind::Register,
            repository_id: None,
        })
    ));
    type_modal_text(&mut app, "existing");
    app.update(map_key(&app, KeyEvent::from(KeyCode::Tab)).unwrap());
    type_modal_text(&mut app, "/tmp/existing");
    let effects = app.update(map_key(&app, KeyEvent::from(KeyCode::Enter)).unwrap());
    assert!(matches!(
        effects.as_slice(),
        [carnet::app::AppEffect::RegisterRepository { name, path }]
            if name == "existing" && path == &PathBuf::from("/tmp/existing")
    ));
}

#[test]
fn repository_selected_actions_keep_the_original_repository_id() {
    let first = RepoEntry {
        id: Uuid::from_u128(31),
        name: "first".into(),
        path: PathBuf::from("/repos/first"),
    };
    let second = RepoEntry {
        id: Uuid::from_u128(32),
        name: "second".into(),
        path: PathBuf::from("/repos/second"),
    };
    let mut app = App::home(vec![first.clone(), second.clone()], None, None);

    app.update(map_key(&app, KeyEvent::new(KeyCode::Char('R'), KeyModifiers::SHIFT)).unwrap());
    assert!(matches!(
        app.dialog,
        Some(Dialog::RepositoryForm {
            kind: RepositoryActionKind::Rename,
            repository_id: Some(id),
        }) if id == first.id
    ));
    app.repository_form.name.clear();
    type_modal_text(&mut app, "renamed");
    app.home.selected = Some(1);
    let rename = app.update(map_key(&app, KeyEvent::from(KeyCode::Enter)).unwrap());
    assert!(matches!(
        rename.as_slice(),
        [carnet::app::AppEffect::RenameRepository { repository_id, name }]
            if *repository_id == first.id && name == "renamed"
    ));
    let renamed = RepoEntry {
        name: "renamed".into(),
        ..first.clone()
    };
    app.update(AppEvent::RepositoryCatalogChanged(
        carnet::app::CatalogSnapshot {
            repositories: vec![renamed.clone(), second.clone()],
            default_repository: None,
            selected_repository: Some(first.id),
        },
    ));

    app.home.selected = Some(0);
    app.update(map_key(&app, KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE)).unwrap());
    assert!(matches!(
        app.dialog,
        Some(Dialog::ConfirmSetDefault { repository_id, .. }) if repository_id == first.id
    ));
    app.home.selected = Some(1);
    let set_default = app.update(map_key(&app, KeyEvent::from(KeyCode::Enter)).unwrap());
    assert!(matches!(
        set_default.as_slice(),
        [carnet::app::AppEffect::SetDefaultRepository { repository_id }]
            if *repository_id == first.id
    ));
    assert_eq!(app.home.default_repository, None);
    app.update(AppEvent::RepositoryCatalogChanged(
        carnet::app::CatalogSnapshot {
            repositories: vec![renamed, second],
            default_repository: Some(first.id),
            selected_repository: Some(first.id),
        },
    ));

    app.home.selected = Some(0);
    app.update(map_key(&app, KeyEvent::new(KeyCode::Char('u'), KeyModifiers::NONE)).unwrap());
    assert!(matches!(
        app.dialog,
        Some(Dialog::ConfirmUnregister { repository_id, .. }) if repository_id == first.id
    ));
    app.home.selected = Some(1);
    let unregister = app.update(map_key(&app, KeyEvent::from(KeyCode::Enter)).unwrap());
    assert!(matches!(
        unregister.as_slice(),
        [carnet::app::AppEffect::UnregisterRepository { repository_id }]
            if *repository_id == first.id
    ));
}

fn mapped_action(app: &App, key: KeyEvent) -> AppAction {
    match map_key(app, key) {
        Some(AppEvent::Action(action)) => action,
        _ => panic!("key did not map to an app action"),
    }
}

fn type_modal_text(app: &mut App, text: &str) {
    for character in text.chars() {
        let event = map_key(
            app,
            KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE),
        )
        .unwrap();
        app.update(event);
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
