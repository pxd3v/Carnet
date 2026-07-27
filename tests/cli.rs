use std::path::PathBuf;

use carnet::cli::Cli;
use clap::{CommandFactory, Parser};

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
