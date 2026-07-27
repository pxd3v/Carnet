use std::{
    io,
    sync::{Arc, Mutex},
    time::Duration,
};

use carnet::{
    app::{AppAction, AppEvent, GlobalAction, Screen},
    catalog::Catalog,
    cli::{Cli, route},
    editor::{Clipboard, ClipboardError},
    git::GitRepo,
    runtime::{RestorationGuard, Runtime, TerminalLifecycle, map_terminal_event},
};
use clap::Parser;
use crossterm::event::Event;
use tempfile::tempdir;

#[test]
fn restoration_guard_restores_on_drop_and_only_once_after_explicit_restore() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    {
        let _guard = RestorationGuard::enter(FakeLifecycle::new(Arc::clone(&calls))).unwrap();
        assert_eq!(*calls.lock().unwrap(), ["enter"]);
    }
    assert_eq!(*calls.lock().unwrap(), ["enter", "restore"]);

    calls.lock().unwrap().clear();
    RestorationGuard::enter(FakeLifecycle::new(Arc::clone(&calls)))
        .unwrap()
        .restore()
        .unwrap();
    assert_eq!(*calls.lock().unwrap(), ["enter", "restore"]);
}

#[test]
fn restoration_guard_attempts_cleanup_when_enter_fails_partway() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let error = RestorationGuard::enter(FakeLifecycle {
        calls: Arc::clone(&calls),
        enter_fails: true,
    })
    .unwrap_err();

    assert_eq!(error.kind(), io::ErrorKind::Other);
    assert_eq!(*calls.lock().unwrap(), ["enter", "restore"]);
}

#[test]
fn bracketed_paste_is_mapped_to_one_editor_transaction() {
    let sandbox = tempdir().unwrap();
    let root = sandbox.path().join("notes");
    GitRepo::initialize(&root).unwrap();
    let mut catalog = Catalog::create_at(sandbox.path().join("catalog.toml"));
    catalog.register("notes", &root).unwrap();
    let launch = route(
        Cli::try_parse_from(["carnet", "pasted.md"]).unwrap(),
        &catalog,
    )
    .unwrap();
    let mut runtime = Runtime::with_clipboard(catalog, launch, Box::new(FailingClipboard));
    runtime.wait_for_idle(Duration::from_secs(3)).unwrap();

    let event = map_terminal_event(runtime.app(), Event::Paste("first\r\nsecond\nthird".into()))
        .expect("paste should map while the editor is active");
    runtime.dispatch(event);
    assert_eq!(editor_text(&runtime), "first\nsecond\nthird");

    runtime.dispatch(AppEvent::Action(AppAction::Global(GlobalAction::Undo)));
    assert_eq!(editor_text(&runtime), "");
}

fn editor_text(runtime: &Runtime) -> String {
    let Screen::Workspace(workspace) = &runtime.app().screen else {
        panic!("expected workspace");
    };
    workspace.editor.as_ref().unwrap().text()
}

#[derive(Debug)]
struct FakeLifecycle {
    calls: Arc<Mutex<Vec<&'static str>>>,
    enter_fails: bool,
}

impl FakeLifecycle {
    fn new(calls: Arc<Mutex<Vec<&'static str>>>) -> Self {
        Self {
            calls,
            enter_fails: false,
        }
    }
}

impl TerminalLifecycle for FakeLifecycle {
    fn enter(&mut self) -> io::Result<()> {
        self.calls.lock().unwrap().push("enter");
        if self.enter_fails {
            Err(io::Error::other("enter failed"))
        } else {
            Ok(())
        }
    }

    fn restore(&mut self) -> io::Result<()> {
        self.calls.lock().unwrap().push("restore");
        Ok(())
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
