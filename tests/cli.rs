use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use carnet::{
    catalog::{Catalog, CatalogError},
    cli::{Cli, CliError, Invocation, Launch, OutputMode, resolve_invocation, route},
};
use clap::{CommandFactory, Parser};
use tempfile::tempdir;

#[test]
fn accepts_an_optional_repository_relative_note_path() {
    let cli = Cli::try_parse_from(["carnet", "notes/today.md"]).unwrap();

    assert_eq!(cli.repo, None);
    assert_eq!(cli.note_path, Some(PathBuf::from("notes/today.md")));
}

#[test]
fn accepts_a_named_repository_with_or_without_a_note_path() {
    let only_repo = Cli::try_parse_from(["carnet", "--repo", "work"]).unwrap();
    let repo_and_note = Cli::try_parse_from(["carnet", "--repo", "work", "roadmap.md"]).unwrap();

    assert_eq!(only_repo.repo.as_deref(), Some("work"));
    assert_eq!(only_repo.note_path, None);
    assert_eq!(repo_and_note.repo.as_deref(), Some("work"));
    assert_eq!(repo_and_note.note_path, Some(PathBuf::from("roadmap.md")));
}

#[test]
fn output_flags_require_a_note_and_conflict_with_each_other() {
    assert!(Cli::try_parse_from(["carnet", "--path"]).is_err());
    assert!(Cli::try_parse_from(["carnet", "--print"]).is_err());
    assert!(Cli::try_parse_from(["carnet", "--path", "--print", "note.md"]).is_err());
}

#[test]
fn resolves_path_output_for_a_note_in_a_named_repository() {
    let sandbox = tempdir().unwrap();
    let repo = sandbox.path().join("notes");
    fs::create_dir(&repo).unwrap();
    let mut catalog = Catalog::create_at(sandbox.path().join("catalog.toml"));
    catalog.register("work", &repo).unwrap();

    let invocation = resolve_invocation(
        Cli::try_parse_from(["carnet", "--repo", "work", "--path", "onboarding.md"]).unwrap(),
        &catalog,
    )
    .unwrap();

    assert!(matches!(
        invocation,
        Invocation::NoteOutput(request)
            if request.mode == OutputMode::Path
                && request.note == Path::new("onboarding.md")
                && request.repository.name == "work"
    ));
}

#[test]
fn resolves_print_output_for_a_note_in_the_default_repository() {
    let sandbox = tempdir().unwrap();
    let repo = sandbox.path().join("notes");
    fs::create_dir(&repo).unwrap();
    let mut catalog = Catalog::create_at(sandbox.path().join("catalog.toml"));
    catalog.register("personal", &repo).unwrap();

    let invocation = resolve_invocation(
        Cli::try_parse_from(["carnet", "--print", "onboarding.md"]).unwrap(),
        &catalog,
    )
    .unwrap();

    assert!(matches!(
        invocation,
        Invocation::NoteOutput(request)
            if request.mode == OutputMode::Print
                && request.note == Path::new("onboarding.md")
                && request.repository.name == "personal"
    ));
}

#[test]
fn resolves_an_ordinary_invocation_to_the_existing_interactive_launch() {
    let sandbox = tempdir().unwrap();
    let repo = sandbox.path().join("notes");
    fs::create_dir(&repo).unwrap();
    let mut catalog = Catalog::create_at(sandbox.path().join("catalog.toml"));
    catalog.register("personal", &repo).unwrap();

    let invocation = resolve_invocation(
        Cli::try_parse_from(["carnet", "onboarding.md"]).unwrap(),
        &catalog,
    )
    .unwrap();

    assert!(matches!(
        invocation,
        Invocation::Interactive(Launch::Repository { repository, note })
            if repository.name == "personal" && note.as_deref() == Some(Path::new("onboarding.md"))
    ));
}

#[test]
fn route_rejects_absolute_and_parent_traversal_note_paths() {
    let sandbox = tempdir().unwrap();
    let repo = sandbox.path().join("notes");
    fs::create_dir(&repo).unwrap();
    let mut catalog = Catalog::create_at(sandbox.path().join("catalog.toml"));
    catalog.register("personal", &repo).unwrap();

    let absolute = route(
        Cli::try_parse_from(["carnet", "/tmp/today.md"]).unwrap(),
        &catalog,
    )
    .unwrap_err();
    let traversal = route(
        Cli::try_parse_from(["carnet", "../outside.md"]).unwrap(),
        &catalog,
    )
    .unwrap_err();

    assert!(matches!(absolute, CliError::AbsoluteNotePath { .. }));
    assert!(matches!(traversal, CliError::TraversalNotePath { .. }));
}

#[test]
fn route_reports_missing_named_registrations() {
    let sandbox = tempdir().unwrap();
    let repo = sandbox.path().join("notes");
    fs::create_dir(&repo).unwrap();
    let mut named_catalog = Catalog::create_at(sandbox.path().join("named.toml"));
    named_catalog.register("personal", &repo).unwrap();

    let named_error = route(
        Cli::try_parse_from(["carnet", "--repo", "work"]).unwrap(),
        &named_catalog,
    )
    .unwrap_err();
    assert!(matches!(
        named_error,
        CliError::Catalog(CatalogError::RepositoryNotFound { .. })
    ));
}

#[test]
fn no_arguments_enters_home_even_without_a_default_repository() {
    let sandbox = tempdir().unwrap();
    let catalog = Catalog::create_at(sandbox.path().join("catalog.toml"));

    let launch = route(Cli::try_parse_from(["carnet"]).unwrap(), &catalog).unwrap();

    assert_eq!(
        launch,
        Launch::Home {
            selected_repository: None,
            pending_note: None,
        }
    );
}

#[test]
fn a_note_without_a_default_enters_home_and_preserves_the_pending_path() {
    let sandbox = tempdir().unwrap();
    let catalog = Catalog::create_at(sandbox.path().join("catalog.toml"));

    let launch = route(
        Cli::try_parse_from(["carnet", "inbox/today.md"]).unwrap(),
        &catalog,
    )
    .unwrap();

    assert_eq!(
        launch,
        Launch::Home {
            selected_repository: None,
            pending_note: Some(PathBuf::from("inbox/today.md")),
        }
    );
}

#[test]
fn process_exits_two_for_a_configuration_failure_before_tui_entry() {
    let home = tempdir().unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_carnet"))
        .env("HOME", home.path())
        .env("XDG_CONFIG_HOME", home.path().join("config"))
        .args(["--repo", "missing"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("repository named \"missing\" is not registered")
    );
}

struct ProcessFixture {
    _sandbox: tempfile::TempDir,
    home: PathBuf,
    repository: PathBuf,
}

impl ProcessFixture {
    fn empty() -> Self {
        let sandbox = tempdir().unwrap();
        let home = sandbox.path().join("home");
        let repository = sandbox.path().join("notes");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&repository).unwrap();
        let repository = fs::canonicalize(repository).unwrap();
        let mut catalog = Catalog::create_at(process_catalog_path(&home));
        catalog.register("personal", &repository).unwrap();
        catalog.save().unwrap();
        Self {
            _sandbox: sandbox,
            home,
            repository,
        }
    }

    fn with_note(path: &str, contents: &[u8]) -> Self {
        let fixture = Self::empty();
        let absolute = fixture.repository.join(path);
        if let Some(parent) = absolute.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(absolute, contents).unwrap();
        fixture
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_carnet"));
        command
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", self.home.join("config"));
        command
    }
}

#[cfg(target_os = "macos")]
fn process_catalog_path(home: &Path) -> PathBuf {
    home.join("Library/Application Support/carnet/catalog.toml")
}

#[cfg(not(target_os = "macos"))]
fn process_catalog_path(home: &Path) -> PathBuf {
    home.join("config/carnet/catalog.toml")
}

#[test]
fn process_prints_a_note_without_entering_the_tui() {
    let fixture =
        ProcessFixture::with_note("onboarding.md", b"\xef\xbb\xbffirst line\r\nsecond line");

    let output = fixture
        .command()
        .args(["--print", "onboarding.md"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(0));
    assert_eq!(output.stdout, b"first line\nsecond line");
    assert!(output.stderr.is_empty());
}

#[test]
fn process_prints_an_absolute_note_path_without_entering_the_tui() {
    let fixture = ProcessFixture::with_note("onboarding.md", b"hello");

    let output = fixture
        .command()
        .args(["--path", "onboarding.md"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        format!("{}\n", fixture.repository.join("onboarding.md").display())
    );
    assert!(output.stderr.is_empty());
}

#[test]
fn process_reports_a_missing_output_note_as_an_operational_failure() {
    let fixture = ProcessFixture::empty();

    let output = fixture
        .command()
        .args(["--print", "missing.md"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("carnet: note does not exist: missing.md")
    );
}

#[test]
fn rendered_long_help_is_the_documented_cli_contract() {
    let help = Cli::command().render_long_help().to_string();

    assert!(help.contains("Usage: carnet [OPTIONS] [NOTE_PATH]"));
    assert!(help.contains("--repo <NAME>"));
}

#[test]
fn documentation_embeds_the_exact_rendered_long_help() {
    let document =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/docs/cli.md")).unwrap();
    let documented_help = document
        .split("<!-- clap-help:start -->\n```text\n")
        .nth(1)
        .and_then(|section| section.split("\n```\n<!-- clap-help:end -->").next())
        .expect("CLI documentation must contain generated help markers");

    assert_eq!(
        documented_help,
        Cli::command().render_long_help().to_string().trim_end()
    );
}
