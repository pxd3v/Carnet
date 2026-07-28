# Command-line interface

`carnet` has no subcommands. With no arguments it opens the repository home and highlights the default registration when one exists. A note path opens in the default repository. If no default exists yet, Carnet enters repository home, preserves the pending note path, and resumes it after a repository becomes the default. `--repo NAME` opens a particular registration, with an optional note path. `--path` and `--print` provide non-interactive access to an existing note for AI agents and shell tools.

## Syntax

```text
carnet [--repo NAME] [--path | --print] [NOTE_PATH]
```

`NOTE_PATH` is optional for interactive use and required with `--path` or `--print`. It is interpreted relative to the selected repository. `--repo NAME` must name a registered repository; with no note path, that repository opens at its tree. When both are omitted, Carnet enters repository home even when the catalog is empty. Repository registrations, renaming, choosing the default, and unregistering are managed from repository home; unregistering never deletes the repository directory.

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

      --path
          Print the absolute path of an existing note and exit

      --print
          Print the contents of an existing text note and exit

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

## Non-interactive note output

`--path NOTE_PATH` prints the absolute path of an existing note followed by one newline. `--print NOTE_PATH` prints the note's logical UTF-8 contents without adding a newline. Printed note text uses the editor's representation: a UTF-8 BOM is omitted and CRLF line endings are normalized to LF while the presence or absence of a final newline is preserved.

The flags are mutually exclusive, require a note path, and support `--repo NAME`. Without `--repo`, they resolve the note in the default repository. Missing notes, directories, symlinks, binary files, and invalid UTF-8 files fail instead of producing output. These modes do not initialize the terminal UI or save, stage, commit, push, or change repository registrations.

## Git behavior

Carnet uses the system `git` executable. After a successful save or trackable file mutation, it stages all non-ignored repository changes with `git add -A` and creates a local commit. It skips empty commits. A file write is not rolled back if a Git commit fails; Carnet reports that outcome and allows a retry with `Ctrl+S`.

Remote synchronization is explicit: `Ctrl+G` runs ordinary `git push` for the open repository in the background. It pushes only commits that already exist; it never saves editor content, creates a commit, chooses a branch, configures a remote, or establishes upstream tracking. The repository's current branch must already have an upstream, and credentials or SSH-agent access must already work non-interactively with system Git. Carnet reports `pushed`, `remote up to date`, or a retryable push failure. A failed push leaves local files and commits untouched.

## Exit codes

- `0`: the session ended cleanly.
- `2`: command-line, catalog/configuration, or note-path validation failed before terminal UI entry.
- `1`: the session ended after an unresolved runtime, write, or Git failure, or non-interactive note output could not be read or written.

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

# Give a path-aware AI agent the absolute path of an existing note.
carnet --path onboarding.md

# Print an existing note for an AI agent or shell pipeline.
carnet --print onboarding.md

# Resolve an existing note in a named repository.
carnet --repo work --path planning/roadmap.md

# Inspect the built-in CLI documentation.
carnet --help
```
