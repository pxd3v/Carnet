use std::{
    fs,
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, SyncSender},
    },
    time::{Duration, Instant},
};

use carnet::{
    app::{AppAction, AppEvent, AppExitStatus, GlobalAction, HomeAction, Screen},
    catalog::{Catalog, CatalogError},
    cli::{Cli, route},
    editor::{Clipboard, ClipboardError, EditorCommand},
    git::GitRepo,
    runtime::{Runtime, WorkerHook, WorkerKind},
    ui::{map_key, render},
};
use clap::Parser;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{Terminal, backend::TestBackend};
use tempfile::tempdir;

#[test]
fn enter_persists_a_missing_notes_default_before_runtime_opens_it() {
    let sandbox = tempdir().unwrap();
    let config = sandbox.path().join("config/catalog.toml");
    let chosen = sandbox.path().join("chosen");
    let catalog = catalog_without_default(&config, sandbox.path(), &chosen);
    let launch = route(
        Cli::try_parse_from(["carnet", "inbox/today.md"]).unwrap(),
        &catalog,
    )
    .unwrap();
    let mut runtime = Runtime::with_clipboard(catalog, launch, Box::new(FailingClipboard));

    let enter = map_key(runtime.app(), KeyEvent::from(KeyCode::Enter)).unwrap();
    runtime.dispatch(enter);
    assert_eq!(runtime.app().home.default_repository, None);
    assert!(runtime.app().pending_request.is_none());
    runtime.wait_for_idle(Duration::from_secs(3)).unwrap();

    assert_eq!(
        Catalog::load_at(config)
            .unwrap()
            .resolve_repo(None)
            .unwrap()
            .name,
        "chosen"
    );
    let Screen::Workspace(workspace) = &runtime.app().screen else {
        panic!("pending note did not open after persistence");
    };
    assert_eq!(
        workspace.current_note.as_deref(),
        Some(Path::new("inbox/today.md"))
    );
}

#[cfg(unix)]
#[test]
fn failed_default_save_keeps_catalog_app_and_pending_note_unchanged() {
    use std::os::unix::fs::PermissionsExt;

    let sandbox = tempdir().unwrap();
    let config_directory = sandbox.path().join("config");
    let config = config_directory.join("catalog.toml");
    let chosen = sandbox.path().join("chosen");
    let catalog = catalog_without_default(&config, sandbox.path(), &chosen);
    let launch = route(
        Cli::try_parse_from(["carnet", "pending.md"]).unwrap(),
        &catalog,
    )
    .unwrap();
    let mut runtime = Runtime::with_clipboard(catalog, launch, Box::new(FailingClipboard));
    fs::set_permissions(&config_directory, fs::Permissions::from_mode(0o555)).unwrap();

    let enter = map_key(runtime.app(), KeyEvent::from(KeyCode::Enter)).unwrap();
    runtime.dispatch(enter);
    runtime.wait_for_idle(Duration::from_secs(3)).unwrap();
    fs::set_permissions(&config_directory, fs::Permissions::from_mode(0o755)).unwrap();

    assert_eq!(runtime.app().home.default_repository, None);
    assert_eq!(
        runtime.app().home.pending_note.as_deref(),
        Some(Path::new("pending.md"))
    );
    assert!(runtime.app().pending_catalog.is_none());
    assert!(runtime.app().pending_request.is_none());
    assert!(matches!(runtime.app().screen, Screen::Home));
    assert!(runtime.app().failures.catalog.is_some());
    assert!(matches!(
        Catalog::load_at(config).unwrap().resolve_repo(None),
        Err(CatalogError::DefaultRepositoryNotSet)
    ));
}

#[test]
fn catalog_dispatch_and_render_continue_while_catalog_worker_is_blocked() {
    let sandbox = tempdir().unwrap();
    let catalog = Catalog::create_at(sandbox.path().join("catalog.toml"));
    let launch = route(Cli::try_parse_from(["carnet"]).unwrap(), &catalog).unwrap();
    let (hook, entered, release) = BlockingHook::new(WorkerKind::Catalog);
    let mut runtime =
        Runtime::with_clipboard_and_hook(catalog, launch, Box::new(FailingClipboard), hook);

    runtime.dispatch(AppEvent::Action(AppAction::Home(
        HomeAction::CreateRepository,
    )));
    runtime.dispatch(AppEvent::Action(AppAction::SetRepositoryFormInput(
        "notes".into(),
    )));
    runtime.dispatch(AppEvent::Action(AppAction::ToggleRepositoryFormField));
    runtime.dispatch(AppEvent::Action(AppAction::SetRepositoryFormInput(
        sandbox.path().join("notes").display().to_string(),
    )));
    let started = Instant::now();
    runtime.dispatch(AppEvent::Action(AppAction::SubmitRepositoryForm));

    assert!(started.elapsed() < Duration::from_millis(200));
    entered.recv_timeout(Duration::from_secs(1)).unwrap();
    render_once(&runtime);
    release.send(()).unwrap();
    runtime.wait_for_idle(Duration::from_secs(3)).unwrap();
    assert_eq!(runtime.app().home.repositories.len(), 1);
}

#[test]
fn editor_and_render_continue_while_clipboard_worker_is_blocked() {
    let sandbox = tempdir().unwrap();
    let root = sandbox.path().join("notes");
    GitRepo::initialize(&root).unwrap();
    fs::write(root.join("note.md"), "base").unwrap();
    let mut catalog = Catalog::create_at(sandbox.path().join("catalog.toml"));
    catalog.register("notes", &root).unwrap();
    let launch = route(
        Cli::try_parse_from(["carnet", "note.md"]).unwrap(),
        &catalog,
    )
    .unwrap();
    let (hook, entered, release) = BlockingHook::new(WorkerKind::ClipboardWrite);
    let mut runtime =
        Runtime::with_clipboard_and_hook(catalog, launch, Box::new(FailingClipboard), hook);
    runtime.wait_for_idle(Duration::from_secs(3)).unwrap();
    runtime.dispatch(AppEvent::Action(AppAction::Global(GlobalAction::SelectAll)));

    let started = Instant::now();
    runtime.dispatch(AppEvent::Action(AppAction::Global(GlobalAction::Copy)));
    assert!(started.elapsed() < Duration::from_millis(200));
    entered.recv_timeout(Duration::from_secs(1)).unwrap();
    runtime.dispatch(AppEvent::Action(AppAction::Editor(EditorCommand::Insert(
        "responsive".into(),
    ))));
    render_once(&runtime);
    assert_eq!(editor_text(&runtime), "responsive");

    release.send(()).unwrap();
    runtime.wait_for_idle(Duration::from_secs(3)).unwrap();
    assert!(runtime.app().failures.clipboard.is_none());
}

#[test]
fn panicking_open_worker_clears_pending_and_produces_failure_exit() {
    let sandbox = tempdir().unwrap();
    let root = sandbox.path().join("notes");
    GitRepo::initialize(&root).unwrap();
    let mut catalog = Catalog::create_at(sandbox.path().join("catalog.toml"));
    catalog.register("notes", &root).unwrap();
    let launch = route(
        Cli::try_parse_from(["carnet", "note.md"]).unwrap(),
        &catalog,
    )
    .unwrap();
    let mut runtime = Runtime::with_clipboard_and_hook(
        catalog,
        launch,
        Box::new(FailingClipboard),
        Arc::new(PanicOnce::new(WorkerKind::OpenWorkspace)),
    );

    runtime.wait_for_idle(Duration::from_secs(3)).unwrap();

    assert!(runtime.app().pending_request.is_none());
    assert!(!runtime.app().failures.runtime.is_empty());
    runtime.dispatch(AppEvent::Action(AppAction::Global(GlobalAction::Quit)));
    assert_eq!(
        runtime.app().quit.final_status,
        Some(AppExitStatus::Failure)
    );
}

#[test]
fn panicking_mutation_worker_clears_pending_and_preserves_disk() {
    let sandbox = tempdir().unwrap();
    let root = sandbox.path().join("notes");
    GitRepo::initialize(&root).unwrap();
    fs::write(root.join("note.md"), "base").unwrap();
    let mut catalog = Catalog::create_at(sandbox.path().join("catalog.toml"));
    catalog.register("notes", &root).unwrap();
    let launch = route(
        Cli::try_parse_from(["carnet", "note.md"]).unwrap(),
        &catalog,
    )
    .unwrap();
    let mut runtime = Runtime::with_clipboard_and_hook(
        catalog,
        launch,
        Box::new(FailingClipboard),
        Arc::new(PanicOnce::new(WorkerKind::Mutation)),
    );
    runtime.wait_for_idle(Duration::from_secs(3)).unwrap();
    runtime.dispatch(AppEvent::Action(AppAction::Editor(EditorCommand::Insert(
        "changed ".into(),
    ))));
    runtime.dispatch(AppEvent::Action(AppAction::Global(GlobalAction::Save)));

    runtime.wait_for_idle(Duration::from_secs(3)).unwrap();

    assert!(runtime.app().pending_mutation.is_none());
    assert!(!runtime.app().failures.runtime.is_empty());
    assert_eq!(fs::read_to_string(root.join("note.md")).unwrap(), "base");
}

#[test]
fn panicking_catalog_and_clipboard_jobs_clear_pending_and_finish() {
    let sandbox = tempdir().unwrap();
    let catalog = Catalog::create_at(sandbox.path().join("catalog.toml"));
    let launch = route(Cli::try_parse_from(["carnet"]).unwrap(), &catalog).unwrap();
    let mut catalog_runtime = Runtime::with_clipboard_and_hook(
        catalog,
        launch,
        Box::new(FailingClipboard),
        Arc::new(PanicOnce::new(WorkerKind::Catalog)),
    );
    catalog_runtime.dispatch(AppEvent::Action(AppAction::Home(
        HomeAction::CreateRepository,
    )));
    catalog_runtime.dispatch(AppEvent::Action(AppAction::SetRepositoryFormInput(
        "notes".into(),
    )));
    catalog_runtime.dispatch(AppEvent::Action(AppAction::ToggleRepositoryFormField));
    catalog_runtime.dispatch(AppEvent::Action(AppAction::SetRepositoryFormInput(
        sandbox.path().join("notes").display().to_string(),
    )));
    catalog_runtime.dispatch(AppEvent::Action(AppAction::SubmitRepositoryForm));
    catalog_runtime
        .wait_for_idle(Duration::from_secs(3))
        .unwrap();
    assert!(catalog_runtime.app().pending_catalog.is_none());
    assert!(catalog_runtime.app().failures.catalog.is_some());

    let root = sandbox.path().join("clipboard-notes");
    GitRepo::initialize(&root).unwrap();
    fs::write(root.join("note.md"), "base").unwrap();
    let mut catalog = Catalog::create_at(sandbox.path().join("clipboard.toml"));
    catalog.register("notes", &root).unwrap();
    let launch = route(
        Cli::try_parse_from(["carnet", "note.md"]).unwrap(),
        &catalog,
    )
    .unwrap();
    let mut clipboard_runtime = Runtime::with_clipboard_and_hook(
        catalog,
        launch,
        Box::new(FailingClipboard),
        Arc::new(PanicOnce::new(WorkerKind::ClipboardWrite)),
    );
    clipboard_runtime
        .wait_for_idle(Duration::from_secs(3))
        .unwrap();
    clipboard_runtime.dispatch(AppEvent::Action(AppAction::Global(GlobalAction::SelectAll)));
    clipboard_runtime.dispatch(AppEvent::Action(AppAction::Global(GlobalAction::Copy)));
    clipboard_runtime
        .wait_for_idle(Duration::from_secs(3))
        .unwrap();
    assert!(clipboard_runtime.app().failures.clipboard.is_some());
}

#[test]
fn create_normalizes_a_missing_target_before_git_and_catalog_work() {
    let sandbox = tempdir().unwrap();
    let catalog = Catalog::create_at(sandbox.path().join("catalog.toml"));
    let launch = route(Cli::try_parse_from(["carnet"]).unwrap(), &catalog).unwrap();
    let mut runtime = Runtime::with_clipboard(catalog, launch, Box::new(FailingClipboard));
    let target = sandbox.path().join("missing").join("..").join("created");

    runtime.dispatch(AppEvent::Action(AppAction::Home(
        HomeAction::CreateRepository,
    )));
    runtime.dispatch(AppEvent::Action(AppAction::SetRepositoryFormInput(
        "created".into(),
    )));
    runtime.dispatch(AppEvent::Action(AppAction::ToggleRepositoryFormField));
    runtime.dispatch(AppEvent::Action(AppAction::SetRepositoryFormInput(
        target.display().to_string(),
    )));
    runtime.dispatch(AppEvent::Action(AppAction::SubmitRepositoryForm));
    runtime.wait_for_idle(Duration::from_secs(3)).unwrap();

    assert!(sandbox.path().join("created/.git").is_dir());
    assert_eq!(
        runtime.app().home.repositories[0].path,
        fs::canonicalize(sandbox.path().join("created")).unwrap()
    );
}

fn catalog_without_default(config: &Path, sandbox: &Path, chosen: &Path) -> Catalog {
    let removed = sandbox.join("removed");
    GitRepo::initialize(&removed).unwrap();
    GitRepo::initialize(chosen).unwrap();
    let mut catalog = Catalog::create_at(config);
    catalog.register("removed", &removed).unwrap();
    catalog.register("chosen", chosen).unwrap();
    catalog.unregister("removed").unwrap();
    catalog.save().unwrap();
    catalog
}

fn render_once(runtime: &Runtime) {
    let backend = TestBackend::new(100, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| render(frame, runtime.app())).unwrap();
}

fn editor_text(runtime: &Runtime) -> String {
    let Screen::Workspace(workspace) = &runtime.app().screen else {
        panic!("expected workspace");
    };
    workspace.editor.as_ref().unwrap().text()
}

struct BlockingHook {
    target: WorkerKind,
    entered: SyncSender<()>,
    release: Mutex<Receiver<()>>,
    fired: AtomicBool,
}

impl BlockingHook {
    fn new(target: WorkerKind) -> (Arc<Self>, Receiver<()>, SyncSender<()>) {
        let (entered_tx, entered_rx) = mpsc::sync_channel(1);
        let (release_tx, release_rx) = mpsc::sync_channel(1);
        (
            Arc::new(Self {
                target,
                entered: entered_tx,
                release: Mutex::new(release_rx),
                fired: AtomicBool::new(false),
            }),
            entered_rx,
            release_tx,
        )
    }
}

impl WorkerHook for BlockingHook {
    fn before_execute(&self, kind: WorkerKind) {
        if kind == self.target && !self.fired.swap(true, Ordering::SeqCst) {
            self.entered.send(()).unwrap();
            assert_ne!(
                self.release
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(2)),
                Err(RecvTimeoutError::Timeout),
                "test did not release blocked worker"
            );
        }
    }
}

struct PanicOnce {
    target: WorkerKind,
    fired: AtomicBool,
}

impl PanicOnce {
    fn new(target: WorkerKind) -> Self {
        Self {
            target,
            fired: AtomicBool::new(false),
        }
    }
}

impl WorkerHook for PanicOnce {
    fn before_execute(&self, kind: WorkerKind) {
        if kind == self.target && !self.fired.swap(true, Ordering::SeqCst) {
            panic!("injected {kind:?} panic");
        }
    }
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
