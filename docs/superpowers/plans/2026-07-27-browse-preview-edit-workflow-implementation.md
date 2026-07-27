# Browse, Preview, and Edit Workflow Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the expanding file tree with current-folder browsing, selection-driven preview, explicit Enter-to-edit, safe Escape-to-browse, and macOS-native word/line/document motions.

**Architecture:** `WorkspaceState` owns a repository-relative browser directory and a selection into that directory's direct children. Note loads carry Preview or Edit purpose so async results preserve the intended focus, while a new browse intent reuses the existing dirty-navigation continuation. Unicode word boundaries live in the editor buffer; key mapping and Ratatui presentation remain thin adapters.

**Tech Stack:** Rust 2024, Ratatui 0.30, Crossterm 0.29, Ropey 1.6, unicode-segmentation 1.12, real filesystem/Git integration tests.

## Global Constraints

- Preview selection must never discard dirty content or apply stale load results.
- Files shows only direct children of one repository-relative directory.
- Enter edits text files; Right/Enter enters folders; Left returns to the parent.
- Escape and Ctrl+B use Save / Discard / Cancel before leaving a dirty editor.
- Option maps from Crossterm `ALT`; Command maps from `SUPER`; Shift extends every motion.
- Enter and Shift+Enter each insert exactly one newline.
- Preserve repository-wide Quick Open, file mutations, wide panes, narrow overlay behavior, and explicit-save Git semantics.

---

### Task 1: Current-folder browser projection

**Files:**
- Modify: `src/app/state.rs`
- Modify: `src/app/update.rs`
- Modify: `src/app/update/requests.rs`
- Modify: `src/ui/workspace.rs`
- Test: `tests/app_transitions.rs`
- Test: `tests/ui_rendering.rs`

**Interfaces:**
- Produce `WorkspaceState::browser_directory: PathBuf` in place of `expanded`.
- Produce shared `directory_entries(tree: &[TreeEntry], directory: &Path) -> &[TreeEntry]` behavior in app and UI projections.

- [ ] Add failing tests proving root projection shows only root children, Right enters a selected folder, Left restores its parent selection, root Left is inert, and empty folders have no selection.
- [ ] Run `cargo test --test app_transitions folder_browser -- --nocapture` and confirm failures reflect flattened expansion behavior.
- [ ] Replace `expanded` with `browser_directory`, initialize it from a loaded note's parent or repository root, and make tree selection index direct children only.
- [ ] Rework Up/Down/Right/Left/Enter directory branches and selection-clamping/reconciliation helpers around the current directory.
- [ ] Add rendering tests for flat direct-child rows and `Files · /` / nested breadcrumb titles; remove indentation and expand/collapse icons.
- [ ] Run the focused app and UI tests until green.

### Task 2: Selection-driven Preview and explicit Editing

**Files:**
- Modify: `src/app/state.rs`
- Modify: `src/app/update.rs`
- Modify: `src/app/update/requests.rs`
- Modify: `src/ui/workspace.rs`
- Test: `tests/app_transitions.rs`
- Test: `tests/ui_keymap.rs`
- Test: `tests/ui_rendering.rs`

**Interfaces:**
- Produce `NoteLoadPurpose::{Preview, Edit}` on `PendingRequest::LoadNote`.
- Produce `request_note_load(path: PathBuf, purpose: NoteLoadPurpose)` and a preview-clearing helper.
- Preserve `NavigationAction::Note` as direct Editing for CLI navigation and Quick Open.

- [ ] Add failing transition tests: Up/Down starts Preview loads; folder/disabled/symlink selection clears the editor; stale preview results are ignored; loaded preview keeps Files focus; Enter promotes a matching load or focuses an already loaded matching file.
- [ ] Add failing keymap tests proving Enter dispatches folder entry/edit through `TreeAction::Open`, while ordinary characters cannot edit during Preview.
- [ ] Add load purpose to state/effects routing, clear stale preview state before reads, validate result path against current selection, and apply focus according to purpose.
- [ ] Make selection movement call one preview reconciliation helper and make Quick Open/CLI note navigation request Edit loads.
- [ ] Render `Loading preview…`, `Preview · path`, `Editing · path`, and empty `Preview`; add focus/title tests.
- [ ] Run focused transition, keymap, effect, and rendering tests until green.

### Task 3: Safe Escape-to-Files continuation

**Files:**
- Modify: `src/app/state.rs`
- Modify: `src/app/update.rs`
- Modify: `src/app/update/mutation.rs`
- Modify: `src/ui/keymap.rs`
- Test: `tests/app_transitions.rs`
- Test: `tests/ui_keymap.rs`
- Test: `tests/e2e.rs`

**Interfaces:**
- Produce `AppAction::BrowseFiles` and `PendingIntent::BrowseFiles`.
- Produce one `request_browse_files()` guard used by Escape and editor-side Ctrl+B.

- [ ] Add failing keymap tests for editor Escape and both sides of Ctrl+B.
- [ ] Add failing transitions for clean browse, dirty Save/Discard/Cancel, pending mutation suppression, matching selection restoration, and no-preview Ctrl+B no-op.
- [ ] Route BrowseFiles through pending intent; show the existing dirty dialog; after discard or successful save, focus Files, show narrow overlay, and select the current file's containing directory.
- [ ] Preserve BrowseFiles across saved-but-uncommitted outcomes and complete it after dismiss; preserve it across external-conflict overwrite/reload and cancel it on conflict cancel.
- [ ] Add an end-to-end browse → preview → edit → dirty Escape → Save → browse test.
- [ ] Run focused transition, keymap, mutation, and e2e tests until green.

### Task 4: macOS-native editor motions and newline contract

**Files:**
- Modify: `src/editor/mod.rs`
- Modify: `src/editor/buffer.rs`
- Modify: `src/ui/keymap.rs`
- Test: `tests/editor.rs`
- Test: `tests/ui_keymap.rs`

**Interfaces:**
- Add `Motion::{WordLeft, WordRight}`.
- Add buffer methods returning grapheme-safe previous-word-start and next-word-end character indices.

- [ ] Add failing editor tests with ASCII, punctuation, whitespace, combining characters, non-Latin words, emoji, existing selection collapse, and Shift extension.
- [ ] Add failing keymap tables for Alt+Left/Right, Super+Left/Right, Super+Up/Down, every Shift combination, Enter, and Shift+Enter.
- [ ] Implement Unicode word motion in `TextBuffer`, reuse existing line/document motions for Super arrows, and ensure modified arrow events do not insert characters.
- [ ] Run `cargo test --test editor word_` and `cargo test --test ui_keymap editor_modifier` until green.

### Task 5: Presentation, documentation, reconciliation, and full verification

**Files:**
- Modify: `src/ui/workspace.rs`
- Modify: `docs/keyboard.md`
- Modify: `tests/tui_snapshots.rs`
- Update: `tests/snapshots/*.snap` workspace-backed snapshots
- Test: `tests/app_transitions.rs`
- Test: `tests/ui_rendering.rs`

**Interfaces:**
- Consume final browser, preview, editing, and motion behavior; no new state contracts.

- [ ] Update the footer's Files and Editor rows with folder drill-down, Enter Edit, Escape Files, Option-word, Command-line/document, and newline hints ordered to fit core navigation within 110 columns.
- [ ] Update `docs/keyboard.md` with current-folder browsing, Preview/Editing, dirty Escape, terminal modifier forwarding, and Enter/Shift+Enter.
- [ ] Add mutation reconciliation tests for current-directory survival, nearest surviving ancestor fallback, target reselection, and preview reconciliation after create/rename/move/delete.
- [ ] Run `cargo test --test app_transitions`, `cargo test --test ui_rendering`, and `cargo test --test tui_snapshots`; review and accept only intended snapshot changes.
- [ ] Run `cargo fmt --all`, `git diff --check`, and `cargo clippy --all-targets --all-features -- -D warnings`.
- [ ] Run `cargo test --all-targets --all-features` and confirm zero failures.
- [ ] Review `git status --short` and the complete diff before any implementation commit.
