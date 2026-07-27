use std::{fs, path::PathBuf};

use carnet::{
    app::{
        App, AppAction, AppEvent, CommitStatus, Dialog, FailureKind, FileActionKind, HomeAction,
        OverlayState, RepositoryActionKind, RepositoryAvailability, RuntimeError, RuntimeOperation,
        Screen, WorkspaceOrigin,
    },
    catalog::RepoEntry,
    editor::{EditorCommand, Motion},
    git::GitRepo,
    ui::render,
    workspace::Workspace,
    workspace::WorkspaceError,
};
use ratatui::{Terminal, backend::TestBackend, style::Color};
use tempfile::{TempDir, tempdir};
use uuid::Uuid;

#[test]
fn home_reports_selection_default_missing_paths_and_pending_note_guidance() {
    let available = tempdir().unwrap();
    let available_path = fs::canonicalize(available.path()).unwrap();
    let first = RepoEntry {
        id: Uuid::from_u128(1),
        name: "available".into(),
        path: available_path,
    };
    let missing = RepoEntry {
        id: Uuid::from_u128(2),
        name: "missing".into(),
        path: available.path().join("gone"),
    };
    let mut app = App::home(
        vec![first.clone(), missing],
        Some(first.id),
        Some(PathBuf::from("draft.md")),
    );
    app.home.selected = Some(1);
    app.home.repository_availability[1] = carnet::app::RepositoryAvailability::MissingOrInvalid;
    available.close().unwrap();

    let output = rendered_text(&app, 110, 24);

    assert!(output.contains("> missing"));
    assert!(output.contains("available"));
    assert!(output.contains("default"));
    let available_row = output
        .lines()
        .find(|line| line.contains("available"))
        .unwrap();
    assert!(available_row.contains("ready"), "{available_row}");
    assert!(output.contains("missing · disabled"));
    assert!(output.contains("Pending note: draft.md"));
    assert!(output.contains("[c] Create"));
    assert!(output.contains("[R] Rename"));
    assert!(output.contains("[d] Default"));
    assert!(output.contains("[u] Unregister"));
    assert!(output.contains("Repository actions"));
}

#[test]
fn empty_home_keeps_pending_note_resume_context_visible() {
    let app = App::home(Vec::new(), None, Some(PathBuf::from("inbox/today.md")));

    let output = rendered_text(&app, 90, 24);

    assert!(output.contains("No repositories registered yet"));
    assert!(
        output.contains("Choose a repository to resume: inbox/today.md"),
        "{output}"
    );
}

#[test]
fn failed_repository_open_marks_the_registration_disabled_in_state() {
    let repository = RepoEntry {
        id: Uuid::from_u128(3),
        name: "gone".into(),
        path: PathBuf::from("/missing/gone"),
    };
    let mut app = App::home(vec![repository.clone()], Some(repository.id), None);
    app.update(AppEvent::Action(AppAction::Home(HomeAction::OpenSelected)));
    let request_id = app.pending_request.as_ref().unwrap().request_id();

    app.update(AppEvent::RuntimeFailed {
        request_id,
        repository_id: repository.id,
        operation: RuntimeOperation::OpenWorkspace,
        error: RuntimeError::Workspace(WorkspaceError::NonCanonicalRoot {
            path: repository.path,
        }),
    });

    assert_eq!(
        app.home.repository_availability,
        vec![RepositoryAvailability::MissingOrInvalid]
    );
}

#[test]
fn editor_projection_uses_highlights_selection_and_cursor_without_mutating_state() {
    let (_sandbox, mut markdown) = workspace_app("note.md", "# Welcome");
    markdown.sidebar.visible = false;
    let Screen::Workspace(workspace) = &mut markdown.screen else {
        panic!("workspace fixture did not open")
    };
    let editor = workspace.editor.as_mut().unwrap();
    editor.apply(EditorCommand::Move {
        motion: Motion::Right,
        extend_selection: true,
    });
    editor.apply(EditorCommand::Move {
        motion: Motion::Right,
        extend_selection: true,
    });
    let before = (editor.text(), editor.cursor(), editor.selection());
    let backend = render_backend(&markdown, 100, 12);

    let first_selected = backend.buffer().cell((1, 1)).unwrap();
    let second_selected = backend.buffer().cell((2, 1)).unwrap();
    let cursor = backend.buffer().cell((3, 1)).unwrap();
    assert_eq!(first_selected.bg, Color::Blue);
    assert_eq!(second_selected.bg, Color::Blue);
    assert_eq!(cursor.bg, Color::Yellow);
    let Screen::Workspace(workspace) = &markdown.screen else {
        panic!("workspace fixture did not open")
    };
    let editor = workspace.editor.as_ref().unwrap();
    assert_eq!((editor.text(), editor.cursor(), editor.selection()), before);

    let (_sandbox, mut markdown) = workspace_app("note.md", "# Welcome");
    markdown.sidebar.visible = false;
    move_cursor_to_end(&mut markdown);
    let markdown_backend = render_backend(&markdown, 100, 12);
    assert_ne!(
        markdown_backend.buffer().cell((1, 1)).unwrap().fg,
        Color::Reset
    );

    let (_sandbox, mut plain) = workspace_app("note.txt", "# Welcome");
    plain.sidebar.visible = false;
    move_cursor_to_end(&mut plain);
    let plain_backend = render_backend(&plain, 100, 12);
    let plain_hash = plain_backend.buffer().cell((1, 1)).unwrap();
    assert_eq!(plain_hash.fg, Color::Reset);
    assert_eq!(plain_hash.bg, Color::Reset);
}

#[test]
fn non_snapshot_overlays_and_failures_expose_only_available_choices() {
    let (_sandbox, mut app) = workspace_app("note.md", "note");
    app.sidebar.visible = false;
    app.overlay = OverlayState::Search {
        query: "needle".into(),
    };
    let search = rendered_text(&app, 100, 20);
    assert!(search.contains("[Enter] Next"));
    assert!(search.contains("[Shift+Enter] Previous"));

    app.overlay = OverlayState::QuickOpen {
        query: "note".into(),
        selected: Some(0),
    };
    let quick_open = rendered_text(&app, 100, 20);
    assert!(quick_open.contains("note.md"));
    assert!(quick_open.contains("[↑/↓] Select"));

    app.overlay = OverlayState::None;
    let Screen::Workspace(workspace) = &app.screen else {
        panic!("workspace fixture did not open")
    };
    let origin = WorkspaceOrigin {
        repository_id: workspace.repository.id,
        repository_root: workspace.workspace.root().to_path_buf(),
    };
    for kind in [
        FileActionKind::NewFile,
        FileActionKind::NewFolder,
        FileActionKind::Rename,
        FileActionKind::Move,
    ] {
        app.dialog = Some(Dialog::FileAction {
            origin: origin.clone(),
            kind,
            target: Some(PathBuf::from("note.md")),
        });
        app.dialog_input = "target.md".into();
        let output = rendered_text(&app, 100, 20);
        assert!(output.contains("target.md_"));
        assert!(output.contains("[Enter] Submit"));
        assert!(output.contains("[Esc] Cancel"));
    }

    app.dialog = Some(Dialog::ConfirmDelete {
        origin: origin.clone(),
        path: PathBuf::from("note.md"),
    });
    let delete = rendered_text(&app, 100, 20);
    assert!(delete.contains("Delete note.md?"));
    assert!(delete.contains("[y/Enter] Delete"));
    assert!(delete.contains("[n/Esc] Cancel"));

    for kind in [FailureKind::Runtime, FailureKind::Write, FailureKind::Git] {
        app.dialog = Some(Dialog::Failure {
            kind,
            message: "operation failed".into(),
        });
        let output = rendered_text(&app, 100, 20);
        assert!(output.contains("operation failed"));
        assert!(output.contains("[Enter/Esc] Dismiss"));
    }
}

#[test]
fn repository_dialogs_render_their_exact_fields_and_safe_choices() {
    let repository = RepoEntry {
        id: Uuid::from_u128(77),
        name: "field-notes".into(),
        path: PathBuf::from("/repos/field-notes"),
    };
    let mut app = App::home(vec![repository], None, None);

    for (action, expected) in [
        (
            HomeAction::CreateRepository,
            [
                "Create repository",
                "Repository name",
                "Directory to create",
                "[Tab] Switch field",
            ],
        ),
        (
            HomeAction::RegisterRepository,
            [
                "Register repository",
                "Repository name",
                "Existing directory",
                "[Tab] Switch field",
            ],
        ),
    ] {
        app.update(AppEvent::Action(AppAction::Home(action)));
        let output = rendered_text(&app, 100, 20);
        for text in expected {
            assert!(output.contains(text), "{output}");
        }
        app.update(AppEvent::Action(AppAction::Dismiss));
    }

    app.update(AppEvent::Action(AppAction::Home(
        HomeAction::RenameSelected,
    )));
    assert!(matches!(
        app.dialog,
        Some(Dialog::RepositoryForm {
            kind: RepositoryActionKind::Rename,
            ..
        })
    ));
    let rename = rendered_text(&app, 100, 20);
    assert!(rename.contains("Rename registration"));
    assert!(rename.contains("Repository name: field-notes_"));
    assert!(rename.contains("[Enter] Rename"));
    app.update(AppEvent::Action(AppAction::Dismiss));

    app.update(AppEvent::Action(AppAction::Home(
        HomeAction::SetDefaultSelected,
    )));
    let default = rendered_text(&app, 100, 20);
    assert!(default.contains("Use field-notes as the default repository?"));
    assert!(default.contains("[y/Enter] Set default"));
    app.update(AppEvent::Action(AppAction::Dismiss));

    app.update(AppEvent::Action(AppAction::Home(
        HomeAction::UnregisterSelected,
    )));
    let unregister = rendered_text(&app, 100, 20);
    assert!(unregister.contains("Remove field-notes from Carnet's registrations?"));
    assert!(unregister.contains("will not be deleted"));
    assert!(unregister.contains("[y/Enter] Unregister"));
}

#[test]
fn status_reports_pending_commit_state() {
    let (_sandbox, mut app) = workspace_app("note.md", "note");
    app.sidebar.visible = false;
    app.status.commit = CommitStatus::Pending;

    let output = rendered_text(&app, 100, 12);

    assert!(
        output.contains("Markdown  ·  saved  ·  Ln 1, Col 1  ·  commit pending"),
        "{output}"
    );

    let Screen::Workspace(workspace) = &mut app.screen else {
        panic!("workspace fixture did not open")
    };
    workspace
        .editor
        .as_mut()
        .unwrap()
        .apply(EditorCommand::Insert("changed ".into()));
    app.status.commit = CommitStatus::Idle;
    app.update(AppEvent::Action(AppAction::Global(
        carnet::app::GlobalAction::Save,
    )));
    let output = rendered_text(&app, 100, 12);
    assert!(output.contains("modified"), "{output}");
    assert!(output.contains("saving"), "{output}");
    assert!(output.contains("commit pending"), "{output}");
}

#[test]
fn editor_viewport_keeps_the_cursor_line_visible() {
    let contents = (1..=20)
        .map(|line| format!("line {line:02}"))
        .collect::<Vec<_>>()
        .join("\n");
    let (_sandbox, mut app) = workspace_app("note.txt", &contents);
    app.sidebar.visible = false;
    move_cursor_to_end(&mut app);

    let output = rendered_text(&app, 70, 8);

    assert!(output.contains("line 20"), "{output}");
    assert!(!output.contains("line 01"), "{output}");
}

#[test]
fn editor_renders_extended_graphemes_atomically_with_whole_cursor_and_selection_style() {
    for grapheme in ["e\u{301}", "👩‍🚀", "🇺🇳"] {
        let text = format!("{grapheme}x");
        let (_sandbox, mut app) = workspace_app("note.txt", &text);
        app.sidebar.visible = false;

        let backend = render_backend(&app, 20, 6);

        let first = backend.buffer().cell((1, 1)).unwrap();
        assert_eq!(first.symbol(), grapheme);
        assert_eq!(first.bg, Color::Yellow);
        let width = unicode_width::UnicodeWidthStr::width(grapheme);

        let Screen::Workspace(workspace) = &mut app.screen else {
            panic!("workspace fixture did not open")
        };
        workspace
            .editor
            .as_mut()
            .unwrap()
            .apply(EditorCommand::Move {
                motion: Motion::Right,
                extend_selection: true,
            });
        let backend = render_backend(&app, 20, 6);
        let first = backend.buffer().cell((1, 1)).unwrap();
        assert_eq!(first.symbol(), grapheme);
        assert_eq!(first.bg, Color::Blue);
        assert_eq!(
            backend.buffer().cell((1 + width as u16, 1)).unwrap().bg,
            Color::Yellow
        );
    }
}

#[test]
fn horizontal_viewport_keeps_a_wide_cursor_grapheme_inside_the_right_edge() {
    let (_sandbox, mut app) = workspace_app("note.txt", "12345678好x");
    app.sidebar.visible = false;
    let Screen::Workspace(workspace) = &mut app.screen else {
        panic!("workspace fixture did not open")
    };
    let editor = workspace.editor.as_mut().unwrap();
    for _ in 0..8 {
        editor.apply(EditorCommand::Move {
            motion: Motion::Right,
            extend_selection: false,
        });
    }

    let backend = render_backend(&app, 8, 5);

    assert_eq!(backend.buffer().cell((5, 1)).unwrap().symbol(), "好");
    assert_eq!(backend.buffer().cell((5, 1)).unwrap().bg, Color::Yellow);
}

#[test]
fn tree_viewport_keeps_an_offscreen_selection_visible() {
    let (_sandbox, mut app) = workspace_app_with_many_files(30);
    let Screen::Workspace(workspace) = &mut app.screen else {
        panic!("workspace fixture did not open")
    };
    workspace.focus = carnet::app::Focus::Tree;
    workspace.tree_selection = Some(25);

    let output = rendered_text(&app, 100, 8);

    assert!(output.contains("file-25.txt"), "{output}");
}

#[test]
fn quick_open_viewport_keeps_selection_and_footer_visible() {
    let (_sandbox, mut app) = workspace_app_with_many_files(30);
    app.sidebar.visible = false;
    app.overlay = OverlayState::QuickOpen {
        query: String::new(),
        selected: Some(25),
    };

    let output = rendered_text(&app, 80, 12);

    assert!(output.contains("file-25.txt"), "{output}");
    assert!(output.contains("[↑/↓] Select"), "{output}");
    assert!(output.contains("[Enter] Open"), "{output}");
}

fn rendered_text(app: &App, width: u16, height: u16) -> String {
    render_backend(app, width, height).to_string()
}

fn render_backend(app: &App, width: u16, height: u16) -> TestBackend {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| render(frame, app)).unwrap();
    terminal.backend().clone()
}

fn move_cursor_to_end(app: &mut App) {
    let Screen::Workspace(workspace) = &mut app.screen else {
        panic!("workspace fixture did not open")
    };
    workspace
        .editor
        .as_mut()
        .unwrap()
        .apply(EditorCommand::Move {
            motion: Motion::DocumentEnd,
            extend_selection: false,
        });
}

fn workspace_app(note_path: &str, contents: &str) -> (TempDir, App) {
    let sandbox = tempdir().unwrap();
    let root = fs::canonicalize(sandbox.path()).unwrap();
    fs::write(root.join(note_path), contents).unwrap();
    let repository = RepoEntry {
        id: Uuid::from_u128(99),
        name: "notes".into(),
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

fn workspace_app_with_many_files(count: usize) -> (TempDir, App) {
    let sandbox = tempdir().unwrap();
    let root = fs::canonicalize(sandbox.path()).unwrap();
    for index in 0..count {
        fs::write(
            root.join(format!("file-{index:02}.txt")),
            format!("file {index}"),
        )
        .unwrap();
    }
    let repository = RepoEntry {
        id: Uuid::from_u128(100),
        name: "many-notes".into(),
        path: root,
    };
    let git = GitRepo::initialize(&repository.path).unwrap();
    let workspace = Workspace::open(repository.clone()).unwrap();
    let tree = workspace.tree().unwrap();
    let note = workspace
        .load_note(
            &workspace
                .resolve_note(PathBuf::from("file-00.txt").as_path())
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
