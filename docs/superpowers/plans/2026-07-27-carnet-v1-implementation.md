# Carnet v1 Implementation Plan

## Global Constraints

Build one Rust binary package for macOS and Linux using current mutually compatible stable crates, Ratatui/Crossterm, the system `git` executable, a custom non-modal editor, Syntect for Markdown/HTML, and explicit-save commits. Commit `Cargo.lock`. Preserve the existing `.superpowers/` directory locally, ignore it, and never ship it.

Keep these interfaces stable:

```rust
Catalog::load() -> Result<Catalog, CatalogError>
Catalog::save(&self) -> Result<(), CatalogError>
Catalog::resolve_repo(&self, name: Option<&str>) -> Result<RepoEntry, CatalogError>

Workspace::open(repo: RepoEntry) -> Result<Workspace, WorkspaceError>
Workspace::resolve_note(&self, path: &Path) -> Result<NotePath, PathError>
Workspace::load_note(&self, path: &NotePath) -> Result<LoadedNote, FileError>
Workspace::apply(operation: FileOperation) -> Result<FileOutcome, FileError>

Editor::from_loaded(note: LoadedNote) -> Editor
Editor::apply(&mut self, command: EditorCommand) -> EditorOutcome
Editor::text(&self) -> String
Editor::is_dirty(&self) -> bool

GitRepo::initialize(path: &Path) -> Result<GitRepo, GitError>
GitRepo::open(path: &Path) -> Result<GitRepo, GitError>
GitRepo::commit_all(intent: CommitIntent) -> Result<CommitOutcome, GitError>

App::update(&mut self, event: AppEvent) -> Vec<AppEffect>
```

Use pure application transitions and a serialized filesystem/Git effect worker. Preserve UTF-8 BOM, LF/CRLF convention, permissions, and final-newline state. Record a load hash and detect external changes before save. Constrain every operation to the canonical repository root, rejecting absolute paths, traversal, `.git`, directory note targets, and symlink escapes.

After each save or trackable mutation, run `git add -A` and commit every staged repository change, including unrelated pending non-ignored changes. Skip empty commits. A completed file mutation survives Git failure and reports `SavedCommitFailed`; global save retries the commit. Disable competing mutations while one is pending.

Implement every behavioral slice with red-green-refactor. Tests exercise Carnet behavior through real temporary files and Git repositories; mock only terminal and clipboard boundaries. Prefer table-driven/property tests. Keep exact snapshots only for contractual layouts. Every task ends with a pristine focused test run and full-suite run and an independently reviewable commit.

The target structure is:

```text
src/main.rs src/cli.rs src/catalog.rs src/git.rs
src/workspace/{mod.rs,paths.rs,tree.rs,files.rs}
src/editor/{mod.rs,buffer.rs,history.rs,search.rs,highlight.rs}
src/app/{mod.rs,state.rs,update.rs,effect.rs}
src/ui/{mod.rs,home.rs,workspace.rs,dialogs.rs}
tests/{cli.rs,catalog.rs,workspace.rs,git_workflow.rs,app_transitions.rs,tui_snapshots.rs}
```

## Task 1: Bootstrap, catalog, CLI, and CLI documentation contract

Create the Rust binary package, central errors as needed, and current stable dependencies. Define Clap routing without subcommands:

- `carnet` opens repository home with the default highlighted.
- `carnet <NOTE_PATH>` opens/prepares a repo-relative note in the default repository.
- `carnet --repo <NAME> [NOTE_PATH]` selects a named registration.
- Support conventional `--help` and `--version`.
- Exit `0` for a clean session, `2` for CLI/config/path failure before TUI entry, and `1` for exit after unresolved runtime/write/Git failure.

Implement a versioned TOML catalog in the OS-standard config directory, with stable UUIDs, unique names, canonical paths, and default repository ID. Write catalog changes by same-directory temporary file, flush, and atomic rename. Implement load/save/resolve plus create/register, rename registration, set default, and unregister-without-disk-deletion model operations. Cover round trips, defaults, duplicate names, missing paths, and atomic replacement.

Add `README.md` basics and `docs/cli.md`. Put Clap-rendered long help between stable generated markers and test exact drift. Document syntax, arguments/options, default selection, config location, path rules, Git semantics, exit codes, and runnable examples.

## Task 2: Path confinement and filesystem primitives

Implement `Workspace`, `NotePath`, visible tree construction, decoding/hashing, atomic writes, and file operations. Requirements:

- Missing notes remain unsaved; first save creates parents and file.
- Show non-ignored paths except `.git`; any UTF-8 text opens; binary/non-UTF-8 files and symlinks are visible but disabled.
- Create file/folder, rename, move, and confirmed delete inside root. Never add `.gitkeep`.
- Preserve UTF-8 BOM, newline convention, existing permissions, and final-newline state. Same-directory temp write, flush/sync, atomic rename.
- Detect external modification or deletion using the load hash and return a conflict supporting Reload, Overwrite, or Cancel at the app layer.

Use property tests for hostile path combinations and real filesystem tests for empty/BOM/LF/CRLF/final-newline/long-line/Unicode/binary/symlink/permissions/failure cases.

## Task 3: System-Git adapter and commit policy

Implement `GitRepo` with ordinary `git init` (respect configured default branch), `git add -A`, staged-change detection, and commits:

- `carnet: create <path>`
- `carnet: update <path>`
- `carnet: move <old> to <new>`
- `carnet: delete <path>`

Use real temp repositories. Prove initial/ordinary commits; unrelated staged, unstaged, and untracked inclusion; ignored exclusion; no-op saves/empty-directory no-change; move/delete; partially staged state; missing identity; failed commit; distinct saved-but-not-committed outcome; and retry. Filesystem writes are never rolled back after Git failure.

## Task 4: Custom editor engine

Build the editor independently from Ratatui with Ropey and grapheme-safe cursor/selection behavior. Implement non-modal movement, Shift selection, insertion/deletion, bracketed paste, process clipboard abstraction with local fallback, undo/redo transactions, select-all, indent/outdent, literal find/navigation, and dirty tracking. Preserve loaded byte metadata on complete undo. Add Syntect Markdown/HTML highlighting cache; all other UTF-8 files are plain.

Use generated Unicode input to prove cursor/selection boundaries and complete undo. Cover multiline selection, clipboard, indentation, search, undo/redo, combining marks, wide characters, and emoji sequences. Test Carnet language selection and representative spans, not Syntect itself.

## Task 5: Pure app state and serialized effects

Implement screens, focus, dialogs, pending work, pure `App::update`, and a serialized per-repository filesystem/Git worker. Model repository home, workspace, editor/tree focus, search, quick-open/sidebar, pending mutation, dirty navigation prompt (Save/Discard/Cancel), external conflict (Reload/Overwrite/Cancel), Git failure/retry, and exit status. Ensure render functions contain no behavior. Test every transition and shortcut through the update layer, including competing-mutation suppression.

## Task 6: Ratatui repository home and workspace UI

Render repository home and the fixed two-pane workspace with persistent tree and one editor. Below the comfortable width, render the tree as an overlay instead of compressing the editor. Add dialogs for repository actions, dirty state, external conflict, Git failure, search, quick open, and file actions.

Implement portable Ctrl shortcuts for save/find/quick-open/sidebar/undo/redo/clipboard/quit. Tree focus: arrows/Enter, `n` file, `Shift+N` folder, `r` rename, `m` move, Delete, Escape. Status shows file type, dirty state, line/column, pending Git changes, and commit outcome. No Vim modes, tabs, preview, mouse-only action, replace, or multi-cursor.

Snapshot only repository home, normal workspace, narrow overlay, dirty prompt, external conflict, and Git failure using Ratatui test backend.

## Task 7: End-to-end workflows, docs, CI, and release packaging

Wire terminal lifecycle, bracketed paste, launch routing, repository home behavior (create/register/open/rename/set-default/unregister), pending-note resume after default selection, and runtime exit status. Add `docs/keyboard.md` with complete home/tree/editor/search/dialog/global shortcuts.

Add macOS/Linux CI for `cargo fmt --check`, Clippy with warnings denied, tests, and coverage reporting without a percentage gate. Tagged releases build Intel/ARM macOS and Linux archives named `carnet-v<version>-<target>.tar.gz` plus SHA-256 checksums. Document prebuilt binaries and `cargo install --path .`; do not publish to crates.io and add no license.

Exercise full workflows through Ratatui's test backend: register two repos/default; open missing CLI path; exact edit/save bytes and Git history; reopen Markdown/HTML highlighting; create/rename/move/delete; dirty navigation; external conflicts; and failed-commit recovery. Run platform-neutral tests locally; CI supplies both OSes.

## Task 8: Release verification and targeted hardening

Run and repair until pristine:

```text
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
documentation drift checks
coverage generation/review
```

Run selective mutation testing (or an available equivalent targeted mutation audit) on editor, path-confinement, and Git-policy modules. Add only behaviorally meaningful tests needed to kill surviving realistic mutations. Verify `.superpowers/` and build/coverage artifacts are untracked. Make the final gate an independently reviewable commit when hardening changes are required; otherwise record verification evidence without an empty product commit.
