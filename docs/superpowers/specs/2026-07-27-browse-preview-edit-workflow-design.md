# Browse, Preview, and Edit Workflow

## Goal

Make Carnet behave like a focused keyboard-first file browser and note editor: browse one directory at a time, preview files by selection, explicitly enter editing, and safely return to browsing without losing changes.

## Interaction model

Carnet has two visible workspace states:

- **Preview** — Files owns keyboard focus. The right pane shows the selected text file but cannot be edited.
- **Editing** — Editor owns keyboard focus. The right pane accepts editing commands.

The editor title identifies the state as `Preview · <path>` or `Editing · <path>`. When there is no previewable selection, the right pane is empty and titled `Preview`.

### Current-folder browser

Files displays only the direct children of one directory. The title is a breadcrumb: `Files · /` at the repository root and `Files · notes/projects` below it. Entries remain directories-first and alphabetically ordered, matching the workspace tree builder.

Keyboard behavior:

- `Up` / `Down` changes selection within the current directory without wrapping.
- Selecting an enabled text file clears any stale right-pane content immediately and starts an asynchronous preview load.
- Selecting a directory, binary/non-UTF-8 file, or symlink clears the preview and cancels ownership of any pending preview result.
- `Right` or `Enter` on a directory enters it, selects its first child, and applies the same preview rule to that child.
- `Left` leaves the current directory. At the parent, Carnet reselects the directory that was just exited, so the preview is cleared. `Left` at the repository root does nothing.
- `Right` on a non-directory does nothing.
- `Enter` on an enabled text file enters Editing. If its preview is loaded, focus changes immediately; if it is still loading, that matching load is promoted to an edit request and focus changes only after it succeeds.
- `Enter` on a disabled file or symlink does nothing.

On narrow terminals, entering Editing hides the Files overlay. Returning to Files shows it again. Wide terminals retain the two-pane layout.

File creation is scoped to the current directory. Rename, move, and delete continue to target the selected entry. Quick Open remains repository-wide and opens its result directly in Editing.

### Returning from Editing

`Escape` requests a return to Files. `Ctrl+B` from Editing follows the same safe path, so it cannot bypass dirty-buffer protection.

- If the editor is clean, Files receives focus immediately and selects the current file in its containing directory.
- If the editor is dirty, Carnet opens the existing Save / Discard / Cancel prompt.
- Save writes the file and then returns to Files. A Git commit failure retains the saved bytes and existing retry affordance; after the failure dialog is dismissed, Files receives focus because the editor is clean.
- Discard restores the last loaded/saved content and returns to Files.
- Cancel closes the prompt and keeps Editing focused.
- While a save or other mutation is pending, duplicate focus transitions are ignored.

The file remains visible as a preview after a clean return to Files because it is the selected text file. Browsing to another entry then replaces or clears that preview.

From Files, Enter is the only ordinary way to enter Editing. `Ctrl+B` keeps its global sidebar-toggle role: from Files it hides the sidebar and focuses Editing when a preview exists; without a preview it does nothing, avoiding invisible Files focus or an empty Editor focus.

## Preview loading and state

`WorkspaceState` replaces expansion state with a repository-relative current-directory path. Tree selection is an index into that directory's direct children rather than a flattened expanded tree.

Note loads carry an explicit purpose: Preview or Edit. Moving selection creates a Preview load; Enter can create or promote an Edit load. Existing request IDs remain the authority for stale-result suppression. A load result applies only when its repository, path, request ID, and current selection still match. Preview results preserve Files focus; Edit results move focus to Editor.

Clearing a preview removes `current_note`, editor state, editor identity, and any matching pending-load ownership immediately. The worker may finish an already-started read, but its result is ignored because it is no longer current.

The right pane displays `Loading preview…` while the selected text file is loading. A failed preview remains cleared, preserves Files focus, and reports the existing contextual runtime failure. It never restores the previously selected file.

Tree refreshes after file operations keep the current directory when it still exists. If it was renamed, moved, or deleted, the browser moves to the nearest surviving ancestor. Selection prefers the affected entry or its replacement, then clamps to the nearest valid child. The preview is reconciled from that final selection.

## Editor keyboard behavior

Existing grapheme-safe arrows and Home/End remain unchanged. The editor adds macOS-native modifier motions when the terminal delivers them:

- `Option+Left`: move to the start of the current or previous Unicode word.
- `Option+Right`: move to the end of the current or next Unicode word.
- `Command+Left` / `Command+Right`: move to the start/end of the current line.
- `Command+Up` / `Command+Down`: move to the start/end of the document.
- Adding Shift to any motion extends the selection from the existing anchor.

Crossterm reports Option as `ALT` and Command as `SUPER`; Carnet maps those modifiers directly. Some terminal applications reserve Command shortcuts, so the keyboard reference will note that the terminal must forward these key combinations.

Both Enter and Shift+Enter insert one newline. This is existing behavior and becomes an explicit tested contract.

Word motions operate on Unicode word boundaries and always land on grapheme boundaries. They skip intervening whitespace and punctuation in the direction of travel. Without Shift, a Left-like motion collapses an existing selection to its start and a Right-like motion collapses it to its end without moving farther, matching current arrow behavior.

## Presentation and shortcut footer

The persistent three-row footer remains. Its Files row changes to describe `Up/Down Select`, `Right/Enter Open Folder`, `Left Parent`, and `Enter Edit`, followed by file operations. Its Editor row adds Option-word, Command-line/document, Escape-to-Files, and Enter/Shift+Enter newline guidance. Labels stay concise enough that navigation and mode-switching commands appear within 110 columns; less common file-operation hints may clip at smaller widths.

Files and Editor borders and footer labels continue to show the active focus. Preview versus Editing in the right-pane title makes the state explicit even when both panes remain visible.

## Error and dirty-state guarantees

- Selection cannot discard dirty content because Files cannot receive focus until Save or Discard resolves.
- Preview requests never start while Editing owns a dirty buffer.
- Stale or out-of-order preview results cannot replace the current selection.
- Enter never focuses an editor for a different file than the selected one.
- Save conflicts retain the existing Reload / Overwrite / Cancel flow before the deferred return to Files completes.
- Canceling any dirty or conflict prompt leaves Editing and its buffer unchanged.

## Testing

Tests will cover:

- direct-child directory projection, breadcrumb paths, entering folders, returning to parents, and root boundaries;
- preview on file selection and preview clearing for directories, disabled files, symlinks, empty directories, and failed loads;
- stale preview suppression during rapid Up/Down navigation;
- Enter-to-edit for loaded and pending previews;
- Escape and `Ctrl+B` return-to-Files behavior for clean, dirty-save, dirty-discard, dirty-cancel, conflict, and saved-but-uncommitted outcomes;
- narrow overlay visibility across Preview and Editing;
- Option and Command motions, Shift extension, selection collapse, Unicode words, punctuation, whitespace, emoji, and line/document boundaries;
- Enter and Shift+Enter newline mapping;
- footer content, active-state styling, and updated normal/narrow snapshots;
- an end-to-end browse → preview → edit → dirty Escape → save → browse workflow.

Implementation will use red-green-refactor and will finish with formatting, Clippy with warnings denied, and the full all-target/all-feature test suite.

## Minimal follow-up opportunities

These are intentionally outside this change:

1. Remember the last directory, file, and cursor position per repository.
2. Add optional soft wrapping for prose-heavy notes.
3. Add a configurable shortcut and path template for today's note.

Tabs, autosave, a command palette, and additional editor modes remain out of scope until daily use demonstrates a concrete need.
