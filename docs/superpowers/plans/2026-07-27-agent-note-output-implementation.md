# Agent Note Output Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [x]`) syntax for tracking.

**Goal:** Add non-interactive `--path` and `--print` commands that resolve or read an existing Carnet note without entering the terminal UI.

**Architecture:** Keep the existing interactive `route` API intact and add a higher-level CLI invocation resolver that returns either an interactive `Launch` or a `NoteOutputRequest`. A focused `note_output` module will open the confined workspace, require an existing text note, and write either its absolute path or logical editor text to an injected writer; `main` will dispatch this before terminal initialization.

**Tech Stack:** Rust 2024, Clap 4 derive API, existing `Catalog` and capability-confined `Workspace`, standard `std::io::Write`, Cargo integration tests.

## Global Constraints

- Preserve Carnet's flag-based, no-subcommand CLI.
- `--path` and `--print` are mutually exclusive and each requires `NOTE_PATH`.
- Both modes support `--repo NAME` and use the default repository otherwise.
- Both modes require an existing readable text note and never initialize the TUI or mutate files, Git, or catalog state.
- `--path` emits the absolute path plus one newline.
- `--print` emits `LoadedNote::text()` without adding a newline.
- CLI and catalog failures exit `2`; note open, validation, read, and write failures exit `1`.

---

### Task 1: Parse and route non-interactive requests

**Files:**
- Modify: `src/cli.rs`
- Test: `tests/cli.rs`

**Interfaces:**
- Produces: `OutputMode::{Path, Print}`.
- Produces: `NoteOutputRequest { repository: RepoEntry, note: PathBuf, mode: OutputMode }`.
- Produces: `Invocation::{Interactive(Launch), NoteOutput(NoteOutputRequest)}`.
- Produces: `resolve_invocation(cli: Cli, catalog: &Catalog) -> Result<Invocation, CliError>`.
- Preserves: `route(cli: Cli, catalog: &Catalog) -> Result<Launch, CliError>` for interactive callers.

- [x] **Step 1: Write failing parser and routing tests**

Add tests proving `--path` and `--print` parse into their modes, accept `--repo`, require `NOTE_PATH`, conflict with each other, and produce a note-output request while ordinary invocations remain interactive.

```rust
#[test]
fn output_flags_require_a_note_and_conflict() {
    assert!(Cli::try_parse_from(["carnet", "--path"]).is_err());
    assert!(Cli::try_parse_from(["carnet", "--print"]).is_err());
    assert!(Cli::try_parse_from(["carnet", "--path", "--print", "note.md"]).is_err());
}

#[test]
fn resolves_non_interactive_note_output() {
    let invocation = resolve_invocation(
        Cli::try_parse_from(["carnet", "--repo", "work", "--path", "onboarding.md"]).unwrap(),
        &catalog,
    )
    .unwrap();
    assert!(matches!(invocation, Invocation::NoteOutput(request)
        if request.mode == OutputMode::Path
            && request.note == Path::new("onboarding.md")
            && request.repository.name == "work"));
}
```

- [x] **Step 2: Run the focused tests and verify RED**

Run: `cargo test --test cli`

Expected: compilation fails because the output fields, types, and `resolve_invocation` do not exist.

- [x] **Step 3: Implement minimal parser and routing support**

Add mutually exclusive, note-requiring Clap flags and the new invocation types. `resolve_invocation` validates the note path, resolves the selected repository for output modes, and delegates unchanged interactive requests to `route`.

```rust
#[arg(long, requires = "note_path", conflicts_with = "print")]
pub path: bool,

#[arg(long, requires = "note_path", conflicts_with = "path")]
pub print: bool,
```

- [x] **Step 4: Run the focused tests and verify GREEN**

Run: `cargo test --test cli`

Expected: parser and routing tests pass except the intentional documentation-help drift, which Task 3 updates.

### Task 2: Emit validated path and text output

**Files:**
- Create: `src/note_output.rs`
- Modify: `src/lib.rs`
- Create: `tests/note_output.rs`

**Interfaces:**
- Consumes: `NoteOutputRequest` and `OutputMode` from `carnet::cli`.
- Produces: `write_note_output(request: NoteOutputRequest, writer: &mut impl Write) -> Result<(), NoteOutputError>`.
- Produces: `NoteOutputError` variants wrapping workspace, path, file, and output-write failures plus a distinct missing-note failure.

- [x] **Step 1: Write failing real-filesystem tests**

Create temporary repositories and test that path mode emits the canonical absolute path plus `\n`, print mode strips a BOM and normalizes CRLF without adding a newline, and missing, binary, invalid UTF-8, directory, and symlink targets fail.

```rust
#[test]
fn path_mode_writes_the_absolute_existing_note_path() {
    let (repository, root) = repository_with_note("onboarding.md", b"hello");
    let mut output = Vec::new();
    write_note_output(request(repository, "onboarding.md", OutputMode::Path), &mut output).unwrap();
    assert_eq!(output, format!("{}\n", root.join("onboarding.md").display()).as_bytes());
}

#[test]
fn print_mode_writes_logical_text_without_adding_a_newline() {
    let (repository, _) = repository_with_note("onboarding.md", b"\xef\xbb\xbffirst\r\nsecond");
    let mut output = Vec::new();
    write_note_output(request(repository, "onboarding.md", OutputMode::Print), &mut output).unwrap();
    assert_eq!(output, b"first\nsecond");
}
```

- [x] **Step 2: Run the focused tests and verify RED**

Run: `cargo test --test note_output`

Expected: compilation fails because `carnet::note_output` does not exist.

- [x] **Step 3: Implement the minimal confined output component**

Open `Workspace`, resolve and load the note, reject `!loaded.is_saved()` as missing, then write either `workspace.root().join(loaded.path().relative())` plus newline or `loaded.text()` verbatim to the supplied writer. Map writer failures to an error that names the selected note.

- [x] **Step 4: Run focused tests and verify GREEN**

Run: `cargo test --test note_output`

Expected: all note-output tests pass.

### Task 3: Dispatch before the TUI and document the contract

**Files:**
- Modify: `src/main.rs`
- Modify: `tests/cli.rs`
- Modify: `README.md`
- Modify: `docs/cli.md`

**Interfaces:**
- Consumes: `resolve_invocation`, `Invocation`, and `write_note_output`.
- Preserves: existing `run_tui` and interactive exit semantics.

- [x] **Step 1: Write failing process-level tests**

Add a helper that creates and saves an isolated catalog under the child process's config directory. Test exact stdout and empty stderr for both output modes, exit `1` plus a `carnet:` error for a missing note, and successful ordinary CLI parsing without terminal entry changes.

```rust
#[test]
fn process_prints_a_note_without_entering_the_tui() {
    let fixture = ProcessFixture::with_note("onboarding.md", "hello agent");
    let output = fixture.command().args(["--print", "onboarding.md"]).output().unwrap();
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(output.stdout, b"hello agent");
    assert!(output.stderr.is_empty());
}
```

- [x] **Step 2: Run process tests and verify RED**

Run: `cargo test --test cli process_`

Expected: output-mode processes still attempt to enter the TUI or fail to emit the expected payload.

- [x] **Step 3: Implement binary dispatch**

Resolve `Invocation` after loading the catalog. For `Invocation::NoteOutput`, lock stdout, call `write_note_output`, return `0` on success, and print `carnet: {error}` plus return `1` on failure. For `Invocation::Interactive`, construct the runtime and retain the existing terminal loop.

- [x] **Step 4: Update generated help and prose documentation**

Regenerate the exact Clap help block in `docs/cli.md`, document both non-interactive flags and their failure behavior, and add concise AI-agent examples to `README.md`.

- [x] **Step 5: Run focused tests and verify GREEN**

Run: `cargo test --test cli`

Expected: process behavior and exact documentation-help drift tests pass.

### Task 4: Refactor and verify the complete change

**Files:**
- Modify only files from Tasks 1-3 if cleanup is needed.

**Interfaces:**
- No new behavior; retain all interfaces defined above.

- [x] **Step 1: Format and run static checks**

Run: `cargo fmt --all -- --check`

Run: `cargo clippy --all-targets --all-features -- -D warnings`

Expected: both commands pass without changes or warnings.

- [x] **Step 2: Run the full test suite**

Run: `cargo test --all-targets --all-features`

Expected: all existing and new tests pass.

- [x] **Step 3: Inspect the final diff**

Run: `git diff --check`

Run: `git status --short`

Expected: no whitespace errors and only the intended plan, CLI, output component, tests, README, and CLI documentation are changed.
