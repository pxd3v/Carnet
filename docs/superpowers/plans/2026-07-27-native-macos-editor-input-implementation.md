# Native macOS Editor Input Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make Carnet's basic single-cursor editing shortcuts and deletion behavior feel native in configured Ghostty on macOS while preserving portable Control aliases and compatibility fallbacks.

**Architecture:** Negotiate Crossterm's enhanced keyboard protocol in the terminal lifecycle, normalize enhanced and recognized legacy key events in the UI keymap, and keep text semantics inside explicit editor commands. Ghostty configuration remains an opt-in documented integration because terminal-owned shortcuts cannot be recovered by Carnet itself.

**Tech Stack:** Rust 2024, Crossterm 0.29, Ratatui 0.30, Ropey, Unicode Segmentation, Insta, Proptest.

## Global Constraints

- Keep Carnet a single-cursor editor; do not add multi-cursor, mouse editing, a command palette, or remappable keys.
- Do not change Files, Preview, Editing, save, or Git semantics.
- Retain every existing Control shortcut as a portable alias.
- All text boundaries must remain Unicode grapheme-safe.
- Unsupported enhanced keyboard reporting must fall back without blocking startup.
- Carnet must never edit the user's Ghostty configuration automatically.
- Finish with `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`, and `cargo test --all-targets --all-features`.

---

### Task 1: Word and line deletion primitives

**Files:**
- Modify: `src/editor/mod.rs`
- Modify: `tests/editor.rs`

**Interfaces:**
- Produces: `EditorCommand::{DeleteWordBackward, DeleteWordForward, DeleteToLineStart, DeleteToLineEnd}`.
- Produces: private `Editor::delete_range(start: usize, end: usize, cursor: usize) -> bool` shared by grapheme, word, and line deletion.
- Consumes: existing `TextBuffer::{previous_word_start,next_word_end,line_start,line_end}` and `Editor::transact`.

- [ ] **Step 1: Write failing focused deletion tests**

Add tests covering word boundaries, line boundaries, selection replacement, and one-step history:

```rust
#[test]
fn word_deletion_handles_unicode_spacing_and_one_step_undo() {
    let mut editor = editor_from("words.md", "one  café, 世界");
    editor.apply(move_command(Motion::DocumentEnd, false));

    assert_eq!(editor.apply(EditorCommand::DeleteWordBackward), EditorOutcome::Changed);
    assert_eq!(editor.text(), "one  café, ");
    assert_eq!(editor.apply(EditorCommand::Undo), EditorOutcome::Changed);
    assert_eq!(editor.text(), "one  café, 世界");

    editor.apply(move_command(Motion::DocumentStart, false));
    assert_eq!(editor.apply(EditorCommand::DeleteWordForward), EditorOutcome::Changed);
    assert_eq!(editor.text(), "  café, 世界");
}

#[test]
fn line_deletion_stops_at_line_boundaries_without_joining_lines() {
    let mut editor = editor_from("lines.md", "alpha\nbeta\ngamma");
    editor.apply(move_command(Motion::Down, false));
    editor.apply(move_command(Motion::LineEnd, false));

    editor.apply(EditorCommand::DeleteToLineStart);
    assert_eq!(editor.text(), "alpha\n\ngamma");
    assert_eq!(editor.apply(EditorCommand::DeleteToLineStart), EditorOutcome::NoChange);
    assert_eq!(editor.apply(EditorCommand::DeleteToLineEnd), EditorOutcome::NoChange);
}

#[test]
fn semantic_deletion_replaces_a_grapheme_safe_selection() {
    let commands = [
        EditorCommand::DeleteWordBackward,
        EditorCommand::DeleteWordForward,
        EditorCommand::DeleteToLineStart,
        EditorCommand::DeleteToLineEnd,
    ];
    for command in commands {
        let mut editor = editor_from("selection.md", "a👩‍🚀b");
        editor.apply(move_command(Motion::Right, false));
        editor.apply(move_command(Motion::Right, true));
        assert_eq!(editor.apply(command), EditorOutcome::Changed);
        assert_eq!(editor.text(), "ab");
        assert_valid_editor_endpoints(&editor);
    }
}
```

- [ ] **Step 2: Run the focused tests and verify RED**

Run:

```bash
cargo test --test editor word_deletion_handles_unicode_spacing_and_one_step_undo
cargo test --test editor line_deletion_stops_at_line_boundaries_without_joining_lines
cargo test --test editor semantic_deletion_replaces_a_grapheme_safe_selection
```

Expected: compilation fails because the four `EditorCommand` variants do not exist.

- [ ] **Step 3: Implement the four commands through one deletion helper**

Extend the command enum and `Editor::apply`, then use one helper so selection and endpoint cleanup cannot diverge:

```rust
fn delete_range(&mut self, start: usize, end: usize, cursor: usize) -> bool {
    if self.selection().is_some() {
        return self.replace_selection("");
    }
    if start == end {
        return false;
    }
    self.buffer.replace(start..end, "");
    self.cursor = self.buffer.boundary_at_or_after(cursor);
    self.anchor = None;
    self.preferred_column = None;
    true
}
```

Route existing Backspace/Delete and the four new commands through `delete_range`. Compute every boundary before mutably borrowing the editor. Keep each call wrapped in one `transact` invocation.

- [ ] **Step 4: Extend generated Unicode coverage**

Increase the Proptest action range and add the four variants so random edits prove endpoint validity and complete undo:

```rust
15 => EditorCommand::DeleteWordBackward,
16 => EditorCommand::DeleteWordForward,
17 => EditorCommand::DeleteToLineStart,
18 => EditorCommand::DeleteToLineEnd,
_ => EditorCommand::SelectAll,
```

- [ ] **Step 5: Run editor tests and verify GREEN**

Run: `cargo test --test editor`

Expected: all editor tests pass, including generated Unicode actions.

---

### Task 2: Native and legacy key normalization

**Files:**
- Modify: `src/ui/keymap.rs`
- Modify: `tests/ui_keymap.rs`

**Interfaces:**
- Consumes: Task 1's four new `EditorCommand` variants.
- Produces: primary Command aliases for Save, Find, Quick Open, Files, Undo/Redo, Copy/Cut/Paste, and Select All.
- Produces: editor mappings for enhanced Option/Command movement and deletion plus legacy `Alt+B/F` and `Control+U`.

- [ ] **Step 1: Write the failing keymap matrices**

Add explicit tests for macOS-primary global actions:

```rust
#[test]
fn command_shortcuts_map_to_native_global_actions() {
    let (_sandbox, app) = workspace_app();
    let cases = [
        ('s', KeyModifiers::SUPER, GlobalAction::Save),
        ('f', KeyModifiers::SUPER, GlobalAction::Find),
        ('p', KeyModifiers::SUPER, GlobalAction::QuickOpen),
        ('b', KeyModifiers::SUPER, GlobalAction::ToggleSidebar),
        ('z', KeyModifiers::SUPER, GlobalAction::Undo),
        ('Z', KeyModifiers::SUPER | KeyModifiers::SHIFT, GlobalAction::Redo),
        ('c', KeyModifiers::SUPER, GlobalAction::Copy),
        ('x', KeyModifiers::SUPER, GlobalAction::Cut),
        ('v', KeyModifiers::SUPER, GlobalAction::Paste),
        ('a', KeyModifiers::SUPER, GlobalAction::SelectAll),
    ];
    for (character, modifiers, expected) in cases {
        assert_eq!(
            mapped_action(&app, KeyEvent::new(KeyCode::Char(character), modifiers)),
            AppAction::Global(expected),
        );
    }
}
```

Add editor matrices for `Option+Shift+Arrow`, `Command+Shift+Arrow`, Option/Command deletion, `Alt+B/F`, and `Control+U`. Assert `Command+Q` and `Command+G` do not become Carnet actions, and mixed unsupported modifiers do not insert text.

- [ ] **Step 2: Run keymap tests and verify RED**

Run: `cargo test --test ui_keymap`

Expected: Command global shortcuts and deletion mappings fail.

- [ ] **Step 3: Normalize the primary modifier without duplicating the keymap**

Add a small helper that accepts exactly one of Control or Super, permits Shift, and rejects Alt or simultaneous Control+Super:

```rust
fn primary_modifier(modifiers: KeyModifiers) -> Option<KeyModifiers> {
    if modifiers.contains(KeyModifiers::ALT) {
        return None;
    }
    match (
        modifiers.contains(KeyModifiers::CONTROL),
        modifiers.contains(KeyModifiers::SUPER),
    ) {
        (true, false) => Some(KeyModifiers::CONTROL),
        (false, true) => Some(KeyModifiers::SUPER),
        _ => None,
    }
}
```

Use it in `global_action`. Map `g` and `q` only when the returned modifier is Control. Preserve dialog and overlay priority by leaving `map_key` ordering unchanged.

- [ ] **Step 4: Add semantic editor movement and deletion mappings**

In `editor_action`, reject ambiguous Control+Alt/Super combinations first. Map:

```rust
(KeyCode::Char('b' | 'B'), ALT) => Motion::WordLeft
(KeyCode::Char('f' | 'F'), ALT) => Motion::WordRight
(KeyCode::Backspace, ALT) => DeleteWordBackward
(KeyCode::Delete, ALT) => DeleteWordForward
(KeyCode::Backspace, SUPER) => DeleteToLineStart
(KeyCode::Delete, SUPER) => DeleteToLineEnd
(KeyCode::Char('u' | 'U'), CONTROL) => DeleteToLineStart
```

Retain the existing arrow mapping order so Super line/document movement wins before Alt word movement and plain arrows. Continue deriving `extend_selection` from Shift for every motion.

- [ ] **Step 5: Run keymap and application routing tests**

Run:

```bash
cargo test --test ui_keymap
cargo test --test app_transitions
```

Expected: all tests pass; dialogs, overlays, Files focus, and Editor focus keep existing priority.

---

### Task 3: Enhanced keyboard terminal lifecycle

**Files:**
- Modify: `src/runtime.rs`
- Modify: `src/main.rs`
- Modify: `tests/terminal.rs`

**Interfaces:**
- Produces: `CrosstermLifecycle::default()` with owned enhanced-keyboard activation state.
- Produces: private `KeyboardEnhancementState::{requested_flags,mark_pushed,take_pop}` used only by the lifecycle.
- Consumes: Crossterm `supports_keyboard_enhancement`, `PushKeyboardEnhancementFlags`, `PopKeyboardEnhancementFlags`, and all four supported `KeyboardEnhancementFlags`.

- [ ] **Step 1: Write failing state and restoration tests**

Keep guard tests in `tests/terminal.rs` and add a private unit-test module in `src/runtime.rs` for the internal state:

```rust
#[test]
fn keyboard_enhancement_falls_back_on_false_or_probe_error() {
    assert!(KeyboardEnhancementState::requested_flags(Ok(false)).is_none());
    assert!(KeyboardEnhancementState::requested_flags(Err(io::Error::other("probe"))).is_none());
}

#[test]
fn keyboard_enhancement_uses_all_flags_and_pops_once() {
    let flags = KeyboardEnhancementState::requested_flags(Ok(true)).unwrap();
    assert!(flags.contains(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES));
    assert!(flags.contains(KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES));
    assert!(flags.contains(KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS));
    assert!(flags.contains(KeyboardEnhancementFlags::REPORT_EVENT_TYPES));

    let mut state = KeyboardEnhancementState::default();
    state.mark_pushed();
    assert!(state.take_pop());
    assert!(!state.take_pop());
}
```

- [ ] **Step 2: Run lifecycle tests and verify RED**

Run: `cargo test keyboard_enhancement_`

Expected: compilation fails because `KeyboardEnhancementState` does not exist.

- [ ] **Step 3: Implement capability negotiation and owned cleanup**

Convert the unit struct to:

```rust
#[derive(Debug, Default)]
pub struct CrosstermLifecycle {
    keyboard: KeyboardEnhancementState,
}
```

After raw mode, call `supports_keyboard_enhancement`. If `requested_flags` returns flags, execute `PushKeyboardEnhancementFlags(flags)` and call `mark_pushed` only after success. During restoration, call `take_pop`; when true, execute `PopKeyboardEnhancementFlags`. Evaluate keyboard cleanup, screen/paste/cursor cleanup, and raw-mode cleanup separately so one error does not suppress later cleanup attempts.

Update `main.rs` to pass `CrosstermLifecycle::default()` into `RestorationGuard::enter`.

- [ ] **Step 4: Run terminal and runtime tests**

Run:

```bash
cargo test --test terminal
cargo test --test runtime
cargo test --test runtime_workers
cargo test --lib runtime::tests
```

Expected: all pass, including partial-entry and exactly-once restoration coverage.

---

### Task 4: Ghostty integration, shortcut presentation, and final verification

**Files:**
- Create: `docs/ghostty.md`
- Modify: `README.md`
- Modify: `docs/keyboard.md`
- Modify: `src/ui/workspace.rs`
- Modify: `tests/ui_rendering.rs`
- Modify: `tests/tui_snapshots.rs`
- Modify: `tests/snapshots/tui_snapshots__wide_workspace_keeps_tree_and_editor_visible.snap`
- Modify: `tests/snapshots/tui_snapshots__narrow_workspace_floats_the_tree_over_a_full_editor.snap`
- Modify other dialog snapshots only if the persistent footer appears in them.

**Interfaces:**
- Consumes: Task 2's final shortcut contract.
- Produces: a copyable Ghostty configuration releasing only editor-owned chords and relocating terminal-output copy/search.
- Produces: platform-specific footer copy without adding rows or changing workspace geometry.

- [ ] **Step 1: Write failing shortcut-copy rendering assertions**

Add focused assertions that macOS copy contains `⌘S Save`, `⌘F Find`, `⌘P Open`, `⌘B Files`, `⇧←→ Select`, `⌥⌫ Word Del`, and `⌘⌫ Line Del`, while portable copy retains `^S`, `^F`, `^P`, and `^B`.

Extract pure helpers taking an explicit style so both variants are testable on every OS:

```rust
#[derive(Clone, Copy)]
enum ShortcutStyle {
    MacOs,
    Portable,
}

fn global_shortcuts(style: ShortcutStyle) -> &'static str;
fn editor_shortcuts(style: ShortcutStyle) -> &'static str;
```

Production chooses `MacOs` with `cfg!(target_os = "macos")`; snapshot tests render with an explicit stable style to remain portable across CI platforms.

- [ ] **Step 2: Run rendering tests and verify RED**

Run: `cargo test --test ui_rendering shortcut`

Expected: fails because native shortcut copy and helpers do not exist.

- [ ] **Step 3: Implement the three-row footer copy**

Keep the existing Global/Files/Editor rows and clipping behavior. Update only shortcut text and make the selection/deletion grammar visible before lower-value hints. Do not add a help overlay or vertical space.

- [ ] **Step 4: Write and validate the Ghostty guide**

Create `docs/ghostty.md` with this exact reassignment set:

```text
keybind = shift+arrow_left=unbind
keybind = shift+arrow_right=unbind
keybind = shift+arrow_up=unbind
keybind = shift+arrow_down=unbind
keybind = super+c=unbind
keybind = super+a=unbind
keybind = super+z=unbind
keybind = super+shift+z=unbind
keybind = super+f=unbind
keybind = super+arrow_left=unbind
keybind = super+arrow_right=unbind
keybind = super+arrow_up=unbind
keybind = super+arrow_down=unbind
keybind = super+shift+arrow_left=unbind
keybind = super+shift+arrow_right=unbind
keybind = super+shift+arrow_up=unbind
keybind = super+shift+arrow_down=unbind
keybind = ctrl+shift+c=copy_to_clipboard
keybind = ctrl+shift+f=start_search
```

Explain global impact, Ghostty config paths, `Command+Shift+,` reload, retained `Command+V`, and fallback Control shortcuts. Link the guide from README and `docs/keyboard.md`.

Validate syntax with the installed Ghostty binary when present by loading a temporary config containing the block and running its config validation command. If Ghostty is unavailable, verify each action and trigger against `ghostty +list-actions` and the official keybinding syntax documented in the guide.

- [ ] **Step 5: Update snapshots and run focused UI/docs checks**

Run:

```bash
INSTA_UPDATE=always cargo test --test tui_snapshots
cargo test --test ui_rendering
cargo test --test tui_snapshots
git diff --check
```

Review every accepted snapshot manually. Only shortcut-footer rows should change.

- [ ] **Step 6: Run the complete verification suite**

Run:

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
```

Expected: all commands exit successfully with no warnings.

- [ ] **Step 7: Commit the complete implementation**

Stage only the plan, source, tests, docs, and reviewed snapshots for this feature:

```bash
git add docs/superpowers/plans/2026-07-27-native-macos-editor-input-implementation.md \
  docs/ghostty.md docs/keyboard.md README.md \
  src/editor/mod.rs src/runtime.rs src/main.rs src/ui/keymap.rs src/ui/workspace.rs \
  tests/editor.rs tests/terminal.rs tests/ui_keymap.rs tests/ui_rendering.rs \
  tests/tui_snapshots.rs tests/snapshots
git commit -m "feat: add native macOS editor input"
```
