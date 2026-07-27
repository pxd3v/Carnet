# Workspace Shortcut Footer and Initial Focus

## Problem

When a repository opens without a note, Carnet selects the first tree entry but gives keyboard focus to the empty editor. Arrow keys and Enter are therefore routed to editor commands that have no visible effect. The existing `Ctrl+B` route to the file tree is documented but undiscoverable on this screen. The workspace also exposes no on-screen keyboard reference.

## Interaction design

Carnet will focus the file tree when a workspace opens without a loaded note. If a note is loaded or prepared, the editor remains the initial focus. The selected pane keeps its existing cyan border, making the active keyboard target visible.

The workspace will reserve three persistent rows at the bottom of the screen, above the existing one-row status line:

1. **Global** — Save, Find, Quick Open, Files, Undo, Redo, Copy, Cut, Paste, Select All, and Quit.
2. **Files** — selection arrows, expand/collapse arrows, Open, New File, New Folder, Rename, Move, Delete, and return to Editor.
3. **Editor** — movement arrows, Shift+arrows selection, newline, indent/outdent, and line start/end.

Every shortcut group remains visible regardless of focus. The label for the active group is highlighted: Files while the tree owns focus and Editor while the editor owns focus. Global remains visually neutral because its shortcuts are shared. Key notation will use concise terminal-friendly labels such as `^S`, `↑↓`, `Enter`, and `S-Tab`.

The footer is a stable three rows rather than wrapping. Each row is rendered as one line and clipped naturally by Ratatui in terminals too narrow to show the full reference. Shortcuts are ordered by importance so the most useful actions remain visible first. This keeps the main layout stable while providing the complete reference on ordinary-width terminals.

## Architecture

Initial focus is application state, so the choice belongs in the workspace-open transition. It will be derived from whether the open result contains a note before that note is moved into the editor.

The shortcut footer is presentation only. A workspace renderer helper will build styled lines from static shortcut descriptions and current focus. It will not duplicate key handling or dispatch actions. The workspace layout will allocate main content, the three-row shortcut footer, and the one-row status line.

The existing keymap remains authoritative. Tree arrows and Enter already map correctly when the tree owns focus, so no new key bindings are required.

## Small-terminal behavior

The main workspace retains a minimum height, and the footer remains three rows. Ratatui clips the right side of each shortcut line when the terminal is narrow. The shortcut order ensures navigation and pane switching appear before less common actions. Existing wide and narrow sidebar behavior remains unchanged.

## Testing

Application transition coverage will prove that:

- a workspace without a note opens with Files focused;
- a workspace with a note opens with Editor focused;
- tree-focused arrows and Enter continue to route through the existing keymap.

Rendering coverage will prove that:

- all three shortcut groups are present in the workspace footer;
- Files is highlighted when the tree owns focus;
- Editor is highlighted when the editor owns focus;
- the existing status line remains at the bottom;
- normal and narrow workspace snapshots reflect the reserved footer height.

Implementation will follow red-green-refactor: add focused failing tests, confirm their expected failures, make the smallest state and rendering changes, then run formatting, Clippy, and the full test suite.
