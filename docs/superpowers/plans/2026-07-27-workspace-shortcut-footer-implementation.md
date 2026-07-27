# Workspace Shortcut Footer Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the file tree immediately keyboard-operable when no note is open and expose every workspace shortcut in a persistent three-row footer.

**Architecture:** Derive initial workspace focus from the presence of a loaded note in the application transition. Keep shortcut discovery in the Ratatui presentation layer as three static styled lines whose active Files or Editor label follows application focus; retain the existing one-row status line below them.

**Tech Stack:** Rust 2024, Ratatui 0.30, Crossterm 0.29, integration tests with Ratatui `TestBackend`, Insta snapshots.

## Global Constraints

- Do not add or change key bindings; `src/ui/keymap.rs` remains authoritative.
- Keep the footer at exactly three rows and the status line at exactly one row.
- Show every Global, Files, and Editor shortcut group regardless of focus.
- Clip shortcut text naturally at narrow widths; do not introduce horizontal scrolling or variable-height wrapping.
- Preserve the existing wide split-pane and narrow overlay behavior.
- Do not perform git write operations.

---

### Task 1: Derive initial focus from loaded-note presence

**Files:**
- Modify: `tests/app_transitions.rs`
- Modify: `src/app/update/requests.rs`

**Interfaces:**
- Consumes: `AppEvent::WorkspaceOpened { note: Option<LoadedNote>, .. }` and `WorkspaceState::focus: Focus`.
- Produces: workspace-open behavior where `note.is_some()` yields `Focus::Editor` and `note.is_none()` yields `Focus::Tree`.

- [ ] **Step 1: Write the failing transition tests**

Add two focused tests using the existing `empty_app` and `app_with_note` fixtures:

```rust
#[test]
fn workspace_without_a_note_opens_with_tree_focus() {
    let (_sandbox, app) = empty_app(176);
    assert_eq!(workspace_focus(&app), carnet::app::Focus::Tree);
}

#[test]
fn workspace_with_a_note_opens_with_editor_focus() {
    let (_sandbox, app) = app_with_note(177, "note.md", "note");
    assert_eq!(workspace_focus(&app), carnet::app::Focus::Editor);
}
```

The first test catches the existing hard-coded `Focus::Editor` branch; the second protects the loaded-note behavior.

- [ ] **Step 2: Run the focused tests and verify the expected failure**

Run:

```bash
cargo test --test app_transitions workspace_without_a_note_opens_with_tree_focus
cargo test --test app_transitions workspace_with_a_note_opens_with_editor_focus
```

Expected: the no-note test fails with Editor instead of Tree; the loaded-note test passes as a characterization test.

- [ ] **Step 3: Implement the minimal state fix**

In `App::handle_workspace_opened`, derive focus before moving `note` into the editor:

```rust
let focus = if note.is_some() {
    Focus::Editor
} else {
    Focus::Tree
};
```

Assign `focus` in `WorkspaceState` instead of `Focus::Editor`.

- [ ] **Step 4: Run the focused transition tests**

Run:

```bash
cargo test --test app_transitions workspace_without_a_note_opens_with_tree_focus
cargo test --test app_transitions workspace_with_a_note_opens_with_editor_focus
```

Expected: both pass.

---

### Task 2: Render the persistent grouped shortcut footer

**Files:**
- Modify: `tests/ui_rendering.rs`
- Modify: `src/ui/workspace.rs`
- Update snapshots: `tests/snapshots/tui_snapshots__wide_workspace_keeps_tree_and_editor_visible.snap`
- Update snapshots: `tests/snapshots/tui_snapshots__narrow_workspace_floats_the_tree_over_a_full_editor.snap`
- Update snapshots: `tests/snapshots/tui_snapshots__dirty_navigation_prompt_exposes_save_discard_and_cancel.snap`
- Update snapshots: `tests/snapshots/tui_snapshots__external_conflict_prompt_exposes_reload_overwrite_and_cancel.snap`
- Update snapshots: `tests/snapshots/tui_snapshots__git_failure_distinguishes_saved_from_committed_and_offers_retry.snap`

**Interfaces:**
- Consumes: `WorkspaceState::focus: Focus` and Ratatui `Frame`/`Rect`.
- Produces: private `render_shortcuts(frame: &mut Frame<'_>, area: Rect, focus: Focus)` presentation helper.

- [ ] **Step 1: Write failing rendering tests**

Add one behavior test that renders a normal-width workspace and checks the hand-authored shortcut labels:

```rust
#[test]
fn workspace_footer_exposes_global_file_and_editor_shortcuts() {
    let (_sandbox, app) = workspace_app("note.md", "note");
    let output = rendered_text(&app, 110, 16);

    assert!(output.contains("Global"), "{output}");
    assert!(output.contains("^S Save"), "{output}");
    assert!(output.contains("^Q Quit"), "{output}");
    assert!(output.contains("Files"), "{output}");
    assert!(output.contains("Enter Open"), "{output}");
    assert!(output.contains("Del Delete"), "{output}");
    assert!(output.contains("Esc Editor"), "{output}");
    assert!(output.contains("Editor"), "{output}");
    assert!(output.contains("S-Arrows Select"), "{output}");
    assert!(output.contains("S-Tab Outdent"), "{output}");
}
```

Add a style test that renders Editor focus, inspects the `Editor` label cell background, changes to Tree focus, re-renders, and proves the `Files` label receives the active cyan/black style while Editor no longer does. Locate label coordinates from the literal three-row layout, not from production helpers.

These tests catch removing a group, omitting important bindings, or highlighting the wrong focus target.

- [ ] **Step 2: Run the rendering tests and verify they fail**

Run:

```bash
cargo test --test ui_rendering workspace_footer
```

Expected: failures because the workspace currently renders only main content plus one status row.

- [ ] **Step 3: Implement the fixed four-row bottom area**

Change the top-level workspace layout to:

```rust
let [main, shortcuts, status] = Layout::vertical([
    Constraint::Min(5),
    Constraint::Length(3),
    Constraint::Length(1),
])
.areas(frame.area());
```

Render three `Line` values in `render_shortcuts`. Use a shared small label helper so Files and Editor labels receive `Style::default().fg(Color::Black).bg(Color::Cyan).add_modifier(Modifier::BOLD)` only when their pane owns focus. Keep Global neutral. Use these exact ordered lines so high-value commands survive clipping:

```text
Global  ^S Save  ^F Find  ^P Open  ^B Files  ^Z Undo  ^Y Redo  ^C Copy  ^X Cut  ^V Paste  ^A All  ^Q Quit
Files   ↑↓ Select  ←→ Fold  Enter Open  n New File  N New Folder  r Rename  m Move  Del Delete  Esc Editor
Editor  Arrows Move  S-Arrows Select  Enter Newline  Tab Indent  S-Tab Outdent  Home/End Line Start/End
```

Do not add borders around the footer; all three rows must remain usable for content.

- [ ] **Step 4: Run the focused rendering tests**

Run:

```bash
cargo test --test ui_rendering workspace_footer
```

Expected: all footer content and focus-style tests pass.

- [ ] **Step 5: Review and update contractual snapshots**

Run:

```bash
cargo test --test tui_snapshots
```

Expected: the two workspace snapshots and three workspace-backed dialog snapshots fail only because three shortcut rows now precede the status row. Review generated `.snap.new` files, then accept those five snapshots with:

```bash
env INSTA_UPDATE=always cargo test --test tui_snapshots
```

Rerun `cargo test --test tui_snapshots` without the update environment variable to verify the accepted snapshots.

---

### Task 3: Verify the complete change

**Files:**
- Verify all modified source, tests, spec, plan, and snapshots.

**Interfaces:**
- Consumes: completed focus and rendering behavior.
- Produces: formatting-, lint-, and test-clean workspace changes.

- [ ] **Step 1: Format and inspect the diff**

Run:

```bash
cargo fmt --all
git diff --check
git diff -- src/app/update/requests.rs src/ui/workspace.rs tests/app_transitions.rs tests/ui_rendering.rs tests/tui_snapshots.rs tests/snapshots docs/superpowers
```

Expected: no whitespace errors; diff contains only the approved behavior, tests, snapshots, and documentation.

- [ ] **Step 2: Run Clippy**

Run:

```bash
cargo clippy --all-targets --all-features -- -D warnings
```

Expected: exit 0 with no warnings.

- [ ] **Step 3: Run the full test suite**

Run:

```bash
cargo test --all-targets --all-features
```

Expected: exit 0 with zero failed tests.

- [ ] **Step 4: Confirm repository state without writing Git metadata**

Run:

```bash
git status --short
```

Expected: only the intended implementation, test, snapshot, spec, and plan files are modified or untracked.
