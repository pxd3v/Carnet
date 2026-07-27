use std::{fs, path::Path, process::Command, time::Duration};

use carnet::{
    app::{
        AppAction, AppEvent, GlobalAction, HomeAction, RepositoryAvailability, RepositoryFormField,
        Screen,
    },
    catalog::Catalog,
    cli::{Cli, Launch, route},
    editor::{Clipboard, ClipboardError, EditorCommand},
    git::GitRepo,
    runtime::Runtime,
};
use clap::Parser;
use tempfile::tempdir;

#[test]
fn create_register_rename_default_and_unregister_are_persisted_and_aligned() {
    let sandbox = tempdir().unwrap();
    let config = sandbox.path().join("catalog.toml");
    let created_path = sandbox.path().join("created");
    let existing_path = sandbox.path().join("existing");
    GitRepo::initialize(&existing_path).unwrap();
    let catalog = Catalog::create_at(&config);
    let launch = route(Cli::try_parse_from(["carnet"]).unwrap(), &catalog).unwrap();
    let mut runtime = Runtime::with_clipboard(catalog, launch, Box::new(FailingClipboard));

    submit_repository_form(
        &mut runtime,
        HomeAction::CreateRepository,
        "created",
        &created_path,
    );
    runtime.wait_for_idle(Duration::from_secs(3)).unwrap();
    assert!(GitRepo::open(&created_path).is_ok());
    assert_home_alignment(&runtime);
    assert_eq!(runtime.app().home.repositories.len(), 1);
    assert_eq!(
        runtime.app().home.default_repository,
        Some(runtime.app().home.repositories[0].id)
    );

    submit_repository_form(
        &mut runtime,
        HomeAction::RegisterRepository,
        "existing",
        &existing_path,
    );
    runtime.wait_for_idle(Duration::from_secs(3)).unwrap();
    assert_home_alignment(&runtime);
    assert_eq!(runtime.app().home.repositories.len(), 2);

    runtime.dispatch(AppEvent::Action(AppAction::Home(HomeAction::Down)));
    runtime.dispatch(AppEvent::Action(AppAction::Home(
        HomeAction::RenameSelected,
    )));
    runtime.dispatch(AppEvent::Action(AppAction::SetRepositoryFormInput(
        "journal".into(),
    )));
    runtime.dispatch(AppEvent::Action(AppAction::SubmitRepositoryForm));
    runtime.wait_for_idle(Duration::from_secs(3)).unwrap();
    assert_eq!(runtime.app().home.repositories[1].name, "journal");
    assert_home_alignment(&runtime);

    runtime.dispatch(AppEvent::Action(AppAction::Home(
        HomeAction::SetDefaultSelected,
    )));
    runtime.dispatch(AppEvent::Action(AppAction::ConfirmRepositoryAction));
    runtime.wait_for_idle(Duration::from_secs(3)).unwrap();
    let journal_id = runtime.app().home.repositories[1].id;
    assert_eq!(runtime.app().home.default_repository, Some(journal_id));

    runtime.dispatch(AppEvent::Action(AppAction::Home(HomeAction::Up)));
    runtime.dispatch(AppEvent::Action(AppAction::Home(
        HomeAction::UnregisterSelected,
    )));
    runtime.dispatch(AppEvent::Action(AppAction::ConfirmRepositoryAction));
    runtime.wait_for_idle(Duration::from_secs(3)).unwrap();
    assert_eq!(runtime.app().home.repositories.len(), 1);
    assert_eq!(runtime.app().home.repositories[0].name, "journal");
    assert_home_alignment(&runtime);
    assert!(
        created_path.is_dir(),
        "unregister must not delete from disk"
    );

    let reloaded = Catalog::load_at(config).unwrap();
    assert_eq!(reloaded.resolve_repo(None).unwrap().name, "journal");
    assert!(reloaded.resolve_repo(Some("created")).is_err());
}

#[test]
fn a_missing_cli_note_opens_as_unsaved_and_clipboard_falls_back_in_process() {
    let sandbox = tempdir().unwrap();
    let root = sandbox.path().join("notes");
    GitRepo::initialize(&root).unwrap();
    configure_identity(&root);
    let mut catalog = Catalog::create_at(sandbox.path().join("catalog.toml"));
    catalog.register("notes", &root).unwrap();
    let launch = route(
        Cli::try_parse_from(["carnet", "missing.md"]).unwrap(),
        &catalog,
    )
    .unwrap();
    assert!(matches!(launch, Launch::Repository { .. }));
    let mut runtime = Runtime::with_clipboard(catalog, launch, Box::new(FailingClipboard));
    runtime.wait_for_idle(Duration::from_secs(3)).unwrap();

    let Screen::Workspace(workspace) = &runtime.app().screen else {
        panic!("expected workspace");
    };
    assert_eq!(
        workspace.current_note.as_deref(),
        Some(Path::new("missing.md"))
    );
    assert_eq!(workspace.editor.as_ref().unwrap().text(), "");
    assert!(!root.join("missing.md").exists());

    runtime.dispatch(AppEvent::Action(AppAction::Editor(EditorCommand::Insert(
        "clipboard text".into(),
    ))));
    runtime.dispatch(AppEvent::Action(AppAction::Global(GlobalAction::SelectAll)));
    runtime.dispatch(AppEvent::Action(AppAction::Global(GlobalAction::Copy)));
    runtime.dispatch(AppEvent::Action(AppAction::Editor(EditorCommand::Insert(
        "replacement ".into(),
    ))));
    runtime.dispatch(AppEvent::Action(AppAction::Global(GlobalAction::Paste)));

    let Screen::Workspace(workspace) = &runtime.app().screen else {
        panic!("expected workspace");
    };
    assert_eq!(
        workspace.editor.as_ref().unwrap().text(),
        "replacement clipboard text"
    );
    assert!(runtime.app().failures.clipboard.is_none());

    runtime.dispatch(AppEvent::Action(AppAction::Global(GlobalAction::Save)));
    runtime.wait_for_idle(Duration::from_secs(3)).unwrap();
    assert_eq!(
        fs::read(root.join("missing.md")).unwrap(),
        b"replacement clipboard text"
    );
    assert_eq!(git_log(&root), "carnet: create missing.md");
}

fn submit_repository_form(runtime: &mut Runtime, action: HomeAction, name: &str, path: &Path) {
    runtime.dispatch(AppEvent::Action(AppAction::Home(action)));
    runtime.dispatch(AppEvent::Action(AppAction::SetRepositoryFormInput(
        name.into(),
    )));
    runtime.dispatch(AppEvent::Action(AppAction::ToggleRepositoryFormField));
    assert_eq!(
        runtime.app().repository_form.active_field,
        RepositoryFormField::Path
    );
    runtime.dispatch(AppEvent::Action(AppAction::SetRepositoryFormInput(
        path.display().to_string(),
    )));
    runtime.dispatch(AppEvent::Action(AppAction::SubmitRepositoryForm));
}

fn assert_home_alignment(runtime: &Runtime) {
    assert_eq!(
        runtime.app().home.repositories.len(),
        runtime.app().home.repository_availability.len()
    );
    assert!(
        runtime
            .app()
            .home
            .repository_availability
            .iter()
            .all(|state| *state == RepositoryAvailability::Available)
    );
    assert!(
        runtime
            .app()
            .home
            .selected
            .is_none_or(|selected| selected < runtime.app().home.repositories.len())
    );
}

fn configure_identity(root: &Path) {
    for args in [
        ["config", "user.name", "Carnet Test"],
        ["config", "user.email", "carnet@example.test"],
    ] {
        let output = Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .unwrap();
        assert!(output.status.success());
    }
}

fn git_log(root: &Path) -> String {
    let output = Command::new("git")
        .args(["log", "-1", "--pretty=%s"])
        .current_dir(root)
        .output()
        .unwrap();
    assert!(output.status.success());
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
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
