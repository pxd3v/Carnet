use std::{fs, path::PathBuf, process::Command};

use carnet::{
    catalog::{Catalog, CatalogError},
    cli::{Cli, CliError, route},
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
fn route_reports_missing_named_and_default_registrations() {
    let sandbox = tempdir().unwrap();
    let repo = sandbox.path().join("notes");
    fs::create_dir(&repo).unwrap();
    let mut named_catalog = Catalog::create_at(sandbox.path().join("named.toml"));
    named_catalog.register("personal", &repo).unwrap();
    let empty_catalog = Catalog::create_at(sandbox.path().join("empty.toml"));

    let named_error = route(
        Cli::try_parse_from(["carnet", "--repo", "work"]).unwrap(),
        &named_catalog,
    )
    .unwrap_err();
    let default_error =
        route(Cli::try_parse_from(["carnet"]).unwrap(), &empty_catalog).unwrap_err();

    assert!(matches!(
        named_error,
        CliError::Catalog(CatalogError::RepositoryNotFound { .. })
    ));
    assert!(matches!(
        default_error,
        CliError::Catalog(CatalogError::DefaultRepositoryNotSet)
    ));
}

#[test]
fn process_exits_two_for_a_configuration_failure_before_tui_entry() {
    let home = tempdir().unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_carnet"))
        .env("HOME", home.path())
        .env("XDG_CONFIG_HOME", home.path().join("config"))
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("default repository is not registered")
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
