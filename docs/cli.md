# Command-line interface

`carnet` has no subcommands. With no arguments it opens the repository home and highlights the default registration when one exists. A note path opens in the default repository. If no default exists yet, Carnet enters repository home, preserves the pending note path, and resumes it after a repository becomes the default. `--repo NAME` opens a particular registration, with an optional note path.

## Syntax

```text
carnet [--repo NAME] [NOTE_PATH]
```

`NOTE_PATH` is optional and is interpreted relative to the selected repository. `--repo NAME` must name a registered repository; with no note path, that repository opens at its tree. When both are omitted, Carnet enters repository home even when the catalog is empty. Repository registrations, renaming, choosing the default, and unregistering are managed from repository home; unregistering never deletes the repository directory.

## Arguments and options

<!-- clap-help:start -->
```text
Open a registered repository or a note within one. Carnet keeps note files in ordinary Git repositories.

Usage: carnet [OPTIONS] [NOTE_PATH]

Arguments:
  [NOTE_PATH]
          Note to open or prepare, relative to the selected repository

Options:
  -r, --repo <NAME>
          Select a registered repository by name

  -h, --help
          Print help (see a summary with '-h')

  -V, --version
          Print version
```
<!-- clap-help:end -->

The marked block is generated from Clap and is covered by an exact drift test; update it whenever the parser changes.

## Configuration and paths

Carnet stores its versioned catalog as `catalog.toml` in the OS-standard application configuration directory:

- macOS: `~/Library/Application Support/carnet/catalog.toml`
- Linux: `$XDG_CONFIG_HOME/carnet/catalog.toml`, or `~/.config/carnet/catalog.toml` when `XDG_CONFIG_HOME` is unset.

Registrations store stable UUIDs, unique names, canonical repository paths, and the default repository UUID. Catalog updates use a flushed, same-directory temporary file followed by an atomic rename.

Repository registrations are stored as canonical paths and must identify an existing Git work-tree root. Creating a repository makes the directory with ordinary `git init`, respecting Git's configured default branch. Note paths must be relative; absolute paths and `..` traversal are rejected before the terminal UI starts. Carnet later also confines note operations to the selected repository, including rejecting `.git` and symlink escapes.

## Git behavior

Carnet uses the system `git` executable. After a successful save or trackable file mutation, it stages all non-ignored repository changes with `git add -A` and creates a commit. It skips empty commits. A file write is not rolled back if a Git commit fails; Carnet reports that outcome and allows a retry.

## Exit codes

- `0`: the session ended cleanly.
- `2`: command-line, catalog/configuration, or note-path validation failed before terminal UI entry.
- `1`: the session ended after an unresolved runtime, write, or Git failure.

## Examples

```sh
# Open the repository home with the default registration highlighted.
carnet

# With no default yet, retain this path while choosing/creating a repository.
carnet inbox/first-note.md

# Open or prepare a note in the default repository.
carnet notes/today.md

# Open or prepare a note in the registered "work" repository.
carnet --repo work planning/roadmap.md

# Open the registered "work" repository without selecting a note.
carnet --repo work

# Inspect the built-in CLI documentation.
carnet --help
```
