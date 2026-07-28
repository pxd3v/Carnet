# Carnet

Carnet is a terminal editor for notes stored in ordinary local Git repositories. It keeps your files portable and records explicit saves as Git commits.

## Requirements

Carnet supports macOS and Linux. It requires:

- the system `git` executable;
- a terminal with standard keyboard input and color support;
- a current stable Rust toolchain when building from source.

Carnet uses the system clipboard when available and keeps a process-local fallback when it is not.

## Install

### Prebuilt releases

Download the archive for your platform from the repository's GitHub Releases page:

- `carnet-v<version>-x86_64-apple-darwin.tar.gz`
- `carnet-v<version>-aarch64-apple-darwin.tar.gz`
- `carnet-v<version>-x86_64-unknown-linux-gnu.tar.gz`
- `carnet-v<version>-aarch64-unknown-linux-gnu.tar.gz`

Each archive has a neighboring `.sha256` checksum. Verify it, extract `carnet`, and place the binary in a directory on your `PATH`.

### Build from source

Build from this checkout with a current stable Rust toolchain:

```sh
cargo install --path .
```

Carnet is not published to crates.io.

## First run

Start at the repository home:

```sh
carnet
```

Press `c` to create a Git repository or `a` to register an existing Git repository. The first registration becomes the default. Creating a repository uses ordinary `git init`; registering requires the selected directory itself to be a Git work-tree root.

## Common workflows

Open or prepare a note in the default repository, or select a named registration:

```sh
carnet notes/today.md
carnet --repo work
carnet --repo work roadmap.md
```

Missing note paths open as unsaved buffers and are created by the first explicit save. `Ctrl+S` writes the file, stages all non-ignored repository changes with `git add -A`, and creates a commit. If the file changed outside Carnet, choose Reload, Overwrite, or Cancel. If the file write succeeds but the Git commit fails, the bytes remain saved and `Ctrl+S` retries only the commit.

Resolve or read an existing note non-interactively when referencing it from an AI agent or shell command:

```sh
carnet --path onboarding.md
carnet --print onboarding.md
carnet --repo work --path planning/roadmap.md
```

`--path` prints the absolute path. `--print` prints the logical UTF-8 note text without adding a newline. Both modes require an existing text note and exit without entering the terminal UI or changing files, Git, or repository registrations.

Repository home can create, register, open, rename, set the default, and unregister repositories. Unregistering never deletes files from disk.

See the [CLI reference](docs/cli.md) for command syntax, configuration, and exit codes, and the [keyboard reference](docs/keyboard.md) for every interaction.
