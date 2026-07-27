use std::{fs, path::Path, process::Command, time::Duration};

use carnet::{
    app::{
        AppAction, AppEvent, AppExitStatus, ConflictChoice, Dialog, DirtyChoice, Focus,
        GlobalAction, HomeAction, NavigationAction, Screen, TreeAction,
    },
    catalog::Catalog,
    cli::{Cli, route},
    editor::{Clipboard, ClipboardError, EditorCommand, HighlightLanguage},
    git::{CommitIntent, GitRepo},
    runtime::Runtime,
    ui,
};
use clap::Parser;
use ratatui::{Terminal, backend::TestBackend, style::Color};
use tempfile::tempdir;

#[test]
fn first_run_registers_two_repositories_resumes_a_missing_note_and_sets_default() {
    let sandbox = tempdir().unwrap();
    let config = sandbox.path().join("catalog.toml");
    let first = sandbox.path().join("first");
    let second = sandbox.path().join("second");
    GitRepo::initialize(&second).unwrap();
    let catalog = Catalog::create_at(&config);
    let launch = route(
        Cli::try_parse_from(["carnet", "inbox/today.md"]).unwrap(),
        &catalog,
    )
    .unwrap();
    let mut harness = Harness::new(catalog, launch);

    assert!(
        harness
            .draw_text()
            .contains("Choose a repository to resume")
    );
    harness.submit_repository(HomeAction::CreateRepository, "personal", &first);
    configure_identity(&first);
    assert_eq!(
        current_note(&harness.runtime),
        Some(Path::new("inbox/today.md"))
    );
    assert!(!first.join("inbox/today.md").exists());

    harness.dispatch(AppEvent::Action(AppAction::Editor(EditorCommand::Insert(
        "# Daily 🚀\nline two\n".into(),
    ))));
    harness.dispatch(AppEvent::Action(AppAction::Global(GlobalAction::Save)));
    assert_eq!(
        fs::read(first.join("inbox/today.md")).unwrap(),
        "# Daily 🚀\nline two\n".as_bytes()
    );
    assert_eq!(git_subject(&first), "carnet: create inbox/today.md");

    harness.dispatch(AppEvent::Action(AppAction::Navigate(
        NavigationAction::Home,
    )));
    harness.submit_repository(HomeAction::RegisterRepository, "work", &second);
    assert_eq!(harness.runtime.app().home.repositories.len(), 2);
    assert_eq!(
        harness.runtime.app().home.repositories.len(),
        harness.runtime.app().home.repository_availability.len()
    );

    harness.dispatch(AppEvent::Action(AppAction::Home(
        HomeAction::SetDefaultSelected,
    )));
    harness.dispatch(AppEvent::Action(AppAction::ConfirmRepositoryAction));
    assert_eq!(
        Catalog::load_at(config)
            .unwrap()
            .resolve_repo(None)
            .unwrap()
            .name,
        "work"
    );
}

#[test]
fn highlighted_markdown_and_html_reopen_while_file_mutations_commit_real_history() {
    let sandbox = tempdir().unwrap();
    let root = sandbox.path().join("notes");
    let git = GitRepo::initialize(&root).unwrap();
    configure_identity(&root);
    fs::write(root.join("readme.md"), "# Heading\n\n*body*").unwrap();
    fs::write(root.join("page.html"), "<h1>Heading</h1>").unwrap();
    git.commit_all(CommitIntent::Create("readme.md".into()))
        .unwrap();
    let (catalog, launch) = launch(&sandbox.path().join("catalog.toml"), &root, "readme.md");
    let mut harness = Harness::new(catalog, launch);

    assert_eq!(
        highlight_language(&harness.runtime),
        HighlightLanguage::Markdown
    );
    assert!(harness.render_has_syntax_color());
    harness.dispatch(AppEvent::Action(AppAction::Navigate(
        NavigationAction::Note("page.html".into()),
    )));
    assert_eq!(
        highlight_language(&harness.runtime),
        HighlightLanguage::Html
    );
    assert!(harness.render_has_syntax_color());

    harness.dispatch(AppEvent::Action(AppAction::Focus(Focus::Tree)));
    harness.dispatch(AppEvent::Action(AppAction::Tree(TreeAction::NewFolder)));
    harness.dispatch(AppEvent::Action(AppAction::SubmitFileAction(
        "archive".into(),
    )));
    assert!(root.join("archive").is_dir());

    harness.dispatch(AppEvent::Action(AppAction::Tree(TreeAction::NewFile)));
    harness.dispatch(AppEvent::Action(AppAction::SubmitFileAction(
        "draft.md".into(),
    )));
    assert!(root.join("draft.md").is_file());
    assert_eq!(current_note(&harness.runtime), Some(Path::new("draft.md")));

    harness.dispatch(AppEvent::Action(AppAction::Tree(TreeAction::Rename)));
    harness.dispatch(AppEvent::Action(AppAction::SubmitFileAction(
        "renamed.md".into(),
    )));
    assert!(!root.join("draft.md").exists());
    assert!(root.join("renamed.md").is_file());

    harness.dispatch(AppEvent::Action(AppAction::Tree(TreeAction::Move)));
    harness.dispatch(AppEvent::Action(AppAction::SubmitFileAction(
        "archive/renamed.md".into(),
    )));
    assert!(root.join("archive/renamed.md").is_file());
    assert_eq!(
        current_note(&harness.runtime),
        Some(Path::new("archive/renamed.md"))
    );

    harness.dispatch(AppEvent::Action(AppAction::Tree(TreeAction::Delete)));
    harness.dispatch(AppEvent::Action(AppAction::ConfirmDelete));
    assert!(!root.join("archive/renamed.md").exists());
    assert_eq!(current_note(&harness.runtime), None);

    let subjects = git_subjects(&root);
    for expected in [
        "carnet: create draft.md",
        "carnet: move draft.md to renamed.md",
        "carnet: move renamed.md to archive/renamed.md",
        "carnet: delete archive/renamed.md",
    ] {
        assert!(
            subjects.iter().any(|subject| subject == expected),
            "{subjects:?}"
        );
    }
}

#[test]
fn dirty_navigation_and_external_conflict_choices_preserve_the_selected_outcome() {
    let sandbox = tempdir().unwrap();
    let root = sandbox.path().join("notes");
    let git = GitRepo::initialize(&root).unwrap();
    configure_identity(&root);
    fs::write(root.join("a.md"), "A").unwrap();
    fs::write(root.join("b.md"), "B").unwrap();
    git.commit_all(CommitIntent::Create("a.md".into())).unwrap();
    let (catalog, launch) = launch(&sandbox.path().join("catalog.toml"), &root, "a.md");
    let mut harness = Harness::new(catalog, launch);

    harness.dispatch(AppEvent::Action(AppAction::Editor(EditorCommand::Insert(
        "dirty ".into(),
    ))));
    harness.dispatch(AppEvent::Action(AppAction::Navigate(
        NavigationAction::Note("b.md".into()),
    )));
    assert!(matches!(
        harness.runtime.app().dialog,
        Some(Dialog::DirtyNavigation)
    ));
    harness.dispatch(AppEvent::DirtyChoice(DirtyChoice::Cancel));
    assert_eq!(current_note(&harness.runtime), Some(Path::new("a.md")));
    assert_eq!(editor_text(&harness.runtime), "dirty A");

    harness.dispatch(AppEvent::Action(AppAction::Navigate(
        NavigationAction::Note("b.md".into()),
    )));
    harness.dispatch(AppEvent::DirtyChoice(DirtyChoice::Discard));
    assert_eq!(editor_text(&harness.runtime), "B");
    harness.dispatch(AppEvent::Action(AppAction::Editor(EditorCommand::Insert(
        "saved ".into(),
    ))));
    harness.dispatch(AppEvent::Action(AppAction::Navigate(
        NavigationAction::Note("a.md".into()),
    )));
    harness.dispatch(AppEvent::DirtyChoice(DirtyChoice::Save));
    assert_eq!(fs::read_to_string(root.join("b.md")).unwrap(), "saved B");
    assert_eq!(editor_text(&harness.runtime), "A");

    harness.dispatch(AppEvent::Action(AppAction::Editor(EditorCommand::Insert(
        "mine ".into(),
    ))));
    fs::write(root.join("a.md"), "external one").unwrap();
    harness.dispatch(AppEvent::Action(AppAction::Global(GlobalAction::Save)));
    assert!(matches!(
        harness.runtime.app().dialog,
        Some(Dialog::ExternalConflict(_))
    ));
    harness.dispatch(AppEvent::ConflictChoice(ConflictChoice::Cancel));
    assert_eq!(editor_text(&harness.runtime), "mine A");
    assert_eq!(
        fs::read_to_string(root.join("a.md")).unwrap(),
        "external one"
    );

    harness.dispatch(AppEvent::Action(AppAction::Global(GlobalAction::Save)));
    harness.dispatch(AppEvent::ConflictChoice(ConflictChoice::Reload));
    assert_eq!(editor_text(&harness.runtime), "external one");
    harness.dispatch(AppEvent::Action(AppAction::Editor(EditorCommand::Insert(
        "mine ".into(),
    ))));
    fs::write(root.join("a.md"), "external two").unwrap();
    harness.dispatch(AppEvent::Action(AppAction::Global(GlobalAction::Save)));
    harness.dispatch(AppEvent::ConflictChoice(ConflictChoice::Overwrite));
    assert_eq!(
        fs::read_to_string(root.join("a.md")).unwrap(),
        "mine external one"
    );
}

#[test]
fn a_failed_commit_keeps_saved_bytes_and_runtime_retry_recovers_clean_exit() {
    let sandbox = tempdir().unwrap();
    let root = sandbox.path().join("notes");
    let git = GitRepo::initialize(&root).unwrap();
    configure_identity(&root);
    fs::write(root.join("note.md"), "base").unwrap();
    git.commit_all(CommitIntent::Create("note.md".into()))
        .unwrap();
    let (catalog, launch) = launch(&sandbox.path().join("catalog.toml"), &root, "note.md");
    let mut harness = Harness::new(catalog, launch);

    git_ok(&root, ["config", "user.name", ""]);
    git_ok(&root, ["config", "user.email", ""]);
    harness.dispatch(AppEvent::Action(AppAction::Editor(EditorCommand::Insert(
        "saved ".into(),
    ))));
    harness.dispatch(AppEvent::Action(AppAction::Global(GlobalAction::Save)));
    assert_eq!(
        fs::read_to_string(root.join("note.md")).unwrap(),
        "saved base"
    );
    assert!(matches!(
        harness.runtime.app().dialog,
        Some(Dialog::SavedCommitFailed { .. })
    ));
    assert!(harness.runtime.app().failures.git.is_some());

    configure_identity(&root);
    harness.dispatch(AppEvent::Action(AppAction::Global(GlobalAction::Save)));
    assert!(harness.runtime.app().failures.git.is_none());
    assert_eq!(git_subject(&root), "carnet: update note.md");
    harness.dispatch(AppEvent::Action(AppAction::Global(GlobalAction::Quit)));
    assert_eq!(
        harness.runtime.app().quit.final_status,
        Some(AppExitStatus::Success)
    );
}

struct Harness {
    runtime: Runtime,
}

impl Harness {
    fn new(catalog: Catalog, launch: carnet::cli::Launch) -> Self {
        let mut runtime = Runtime::with_clipboard(catalog, launch, Box::new(FailingClipboard));
        runtime.wait_for_idle(Duration::from_secs(3)).unwrap();
        let harness = Self { runtime };
        let _ = harness.draw_text();
        harness
    }

    fn dispatch(&mut self, event: AppEvent) {
        self.runtime.dispatch(event);
        self.runtime.wait_for_idle(Duration::from_secs(3)).unwrap();
        let _ = self.draw_text();
    }

    fn submit_repository(&mut self, action: HomeAction, name: &str, path: &Path) {
        self.dispatch(AppEvent::Action(AppAction::Home(action)));
        self.dispatch(AppEvent::Action(AppAction::SetRepositoryFormInput(
            name.into(),
        )));
        self.dispatch(AppEvent::Action(AppAction::ToggleRepositoryFormField));
        self.dispatch(AppEvent::Action(AppAction::SetRepositoryFormInput(
            path.display().to_string(),
        )));
        self.dispatch(AppEvent::Action(AppAction::SubmitRepositoryForm));
    }

    fn draw_text(&self) -> String {
        self.render_backend().to_string()
    }

    fn render_has_syntax_color(&self) -> bool {
        self.render_backend()
            .buffer()
            .content()
            .iter()
            .any(|cell| matches!(cell.fg, Color::Rgb(_, _, _)))
    }

    fn render_backend(&self) -> TestBackend {
        let backend = TestBackend::new(110, 28);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| ui::render(frame, self.runtime.app()))
            .unwrap();
        terminal.backend().clone()
    }
}

fn launch(config: &Path, root: &Path, note: &str) -> (Catalog, carnet::cli::Launch) {
    let mut catalog = Catalog::create_at(config);
    catalog.register("notes", root).unwrap();
    let launch = route(Cli::try_parse_from(["carnet", note]).unwrap(), &catalog).unwrap();
    (catalog, launch)
}

fn current_note(runtime: &Runtime) -> Option<&Path> {
    let Screen::Workspace(workspace) = &runtime.app().screen else {
        return None;
    };
    workspace.current_note.as_deref()
}

fn editor_text(runtime: &Runtime) -> String {
    let Screen::Workspace(workspace) = &runtime.app().screen else {
        panic!("expected workspace");
    };
    workspace.editor.as_ref().unwrap().text()
}

fn highlight_language(runtime: &Runtime) -> HighlightLanguage {
    let Screen::Workspace(workspace) = &runtime.app().screen else {
        panic!("expected workspace");
    };
    workspace.editor.as_ref().unwrap().highlight_language()
}

fn configure_identity(root: &Path) {
    git_ok(root, ["config", "user.name", "Carnet Test"]);
    git_ok(root, ["config", "user.email", "carnet@example.test"]);
}

fn git_ok<const N: usize>(root: &Path, args: [&str; N]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn git_subject(root: &Path) -> String {
    git_subjects(root).into_iter().next().unwrap()
}

fn git_subjects(root: &Path) -> Vec<String> {
    let output = Command::new("git")
        .args(["log", "--pretty=%s"])
        .current_dir(root)
        .output()
        .unwrap();
    assert!(output.status.success());
    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect()
}

struct FailingClipboard;

impl Clipboard for FailingClipboard {
    fn read_text(&mut self) -> Result<String, ClipboardError> {
        Err(ClipboardError::Unavailable)
    }

    fn write_text(&mut self, _text: &str) -> Result<(), ClipboardError> {
        Err(ClipboardError::Unavailable)
    }
}
