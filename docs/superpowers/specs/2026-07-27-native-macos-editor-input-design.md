# Native macOS Editor Input

## Goal

Make basic text editing in Carnet feel immediate and familiar in Ghostty on macOS. A user should be able to move, select, delete, copy, cut, paste, undo, save, find, and open notes with the same single-cursor shortcut grammar they use in Zed or VS Code.

This change addresses the editor's input mechanics. It does not redesign the existing Files, Preview, and Editing workflow.

## Success criteria

- `Option`, `Command`, and `Shift` combinations work without adding `Control` as a workaround.
- Every movement can extend an existing selection with `Shift`.
- Copy, cut, paste, select all, undo, redo, save, find, quick open, and Files have macOS-primary shortcuts where Ghostty can forward them.
- Word and line deletion behave like ordinary macOS editors and remain Unicode-safe.
- Existing `Control` shortcuts remain available as portable aliases.
- Carnet starts and remains usable when a terminal does not support enhanced keyboard reporting.
- A documented Ghostty integration releases the keys that Ghostty otherwise consumes before Carnet can observe them.

## Scope

### Included

- Progressive keyboard enhancement negotiation through Crossterm.
- Normalization of enhanced and useful legacy terminal events into Carnet actions.
- Native macOS movement, selection, clipboard, history, and application shortcuts.
- Deletion by word and to line boundaries.
- A focused Ghostty configuration guide.
- Platform-appropriate shortcut hints and keyboard documentation.

### Excluded

- Multiple cursors.
- Mouse-based editing or selection.
- A command palette.
- User-remappable Carnet keybindings.
- Duplicate, move, or transform-line commands.
- Changes to repository browsing, preview behavior, saving semantics, or Git operations.

## User-visible editing contract

Carnet remains a single-cursor editor. The primary macOS bindings and retained portable aliases are:

| Action | Primary macOS shortcut | Portable alias |
| --- | --- | --- |
| Move by grapheme or visual line | Arrow keys | — |
| Extend any movement | `Shift` plus that movement | — |
| Move or select by word | `Option+Left/Right` | — |
| Move or select to a line boundary | `Command+Left/Right` | `Home/End` |
| Move or select to a document boundary | `Command+Up/Down` | — |
| Delete the previous or next word | `Option+Backspace/Delete` | — |
| Delete to the start or end of the line | `Command+Backspace/Delete` | — |
| Copy, cut, paste, select all | `Command+C/X/V/A` | `Control+C/X/V/A` |
| Undo and redo | `Command+Z`, `Command+Shift+Z` | `Control+Z`, `Control+Y` |
| Save | `Command+S` | `Control+S` |
| Find | `Command+F` | `Control+F` |
| Quick open | `Command+P` | `Control+P` |
| Show or focus Files | `Command+B` | `Control+B` |

`Command+Q` remains Ghostty's application quit shortcut, so Carnet keeps `Control+Q`. `Command+G` retains its macOS find-next meaning outside Carnet, so Carnet keeps `Control+G` for Git push.

### Selection semantics

- Adding `Shift` to any supported movement establishes a fixed anchor and extends the selection to the new cursor position.
- Reversing direction with `Shift` shrinks the selection through the same anchor and can continue extending on the other side.
- An unmodified left-like movement collapses a selection to its start. An unmodified right-like movement collapses it to its end.
- An unmodified vertical, line-boundary, or document-boundary movement starts from the active cursor endpoint, performs that movement, and clears the selection.
- Inserting text, pasting, Backspace, Delete, word deletion, or line-boundary deletion replaces the current selection.
- Copy with no selection does nothing. Cut with no selection does not delete content.

### Deletion semantics

- `Option+Backspace` removes from the cursor through the start of the previous Unicode word. Intervening punctuation or whitespace is included so repeated presses make useful progress.
- `Option+Delete` removes from the cursor through the end of the next Unicode word with the same punctuation and whitespace behavior.
- `Command+Backspace` removes from the cursor to the current line start.
- `Command+Delete` removes from the cursor to the current line end.
- At the corresponding boundary, the command is a no-op. It does not unexpectedly join lines.
- Each deletion is one editor transaction and therefore one undo step.

## Terminal input architecture

### Capability negotiation

`CrosstermLifecycle` will query `supports_keyboard_enhancement` after raw mode is enabled. On supporting terminals it will push `DISAMBIGUATE_ESCAPE_CODES`, `REPORT_ALL_KEYS_AS_ESCAPE_CODES`, `REPORT_ALTERNATE_KEYS`, and `REPORT_EVENT_TYPES`. This is Crossterm's complete supported enhancement set and provides unambiguous modified keys plus press, repeat, and release kinds. The lifecycle will remember whether the push succeeded and pop exactly one level during restoration.

Keyboard enhancement cleanup joins the existing cursor, bracketed-paste, alternate-screen, and raw-mode cleanup. Restoration must still attempt all owned cleanup when entering the terminal fails partway, normal execution returns, or unwinding drops the guard.

An unsupported capability query, a negative result, or a query error falls back to legacy input. It does not prevent Carnet from opening. A failure after Carnet has positively begun changing terminal state follows the existing terminal-runtime error path and still attempts restoration.

### Event normalization

Terminal events are translated into semantic Carnet actions before editor mutation. The normalizer is responsible for terminal representation, while `Editor` remains responsible for text semantics.

The normalizer accepts:

- Enhanced key events containing `SUPER`, `ALT`, `SHIFT`, and their combinations.
- Existing `Control` shortcuts.
- Ghostty's legacy `Esc+B` and `Esc+F` encodings for `Option+Left/Right` when enhanced reporting is unavailable or those Ghostty bindings remain active.
- Ghostty's `Control+U` encoding for `Command+Backspace` as a compatibility input while the native enhanced event remains the documented contract.

Global actions are matched by semantic primary modifier rather than by duplicating each complete keymap. Editor movements and deletions use the same modifier interpretation, including `Shift` extension. Unsupported modifier combinations are ignored and must never insert the unmodified character accidentally.

### Editor primitives

The editor command model gains four explicit mutations:

- delete word backward;
- delete word forward;
- delete to line start;
- delete to line end.

They use `TextBuffer` boundary helpers, the existing selection replacement path, and the existing transaction/history mechanism. Word boundaries must land on grapheme boundaries. No command bypasses selection replacement, history limits, search-navigation reset, or syntax-highlight invalidation.

## Ghostty integration

Ghostty defines terminal-level actions for several requested chords. Those actions run before a child TUI can receive keyboard data, so protocol negotiation alone cannot recover the consumed keys.

`docs/ghostty.md` will provide a copyable configuration block that:

- releases `Shift+Arrow` so Carnet can extend selections;
- releases `Command+C/A/Z/Shift+Z/F` for Carnet's clipboard, selection, history, and find actions;
- releases `Command+Arrow` and `Command+Shift+Arrow` for line/document movement and selection;
- leaves `Command+V` as Ghostty paste because bracketed paste already reaches Carnet as one editor transaction;
- leaves `Option+Arrow` compatible with Ghostty's `Esc+B/F` defaults, which Carnet normalizes;
- provides replacement Ghostty bindings for terminal-output copy and scrollback search on `Control+Shift+C/F`.

The guide will explain that Ghostty configuration is global, show how to reload it, and list the exact behaviors being reassigned. Carnet will not edit the user's Ghostty configuration automatically.

The README and keyboard reference will link to the guide. The keyboard reference will distinguish guaranteed portable shortcuts from macOS shortcuts that require the Ghostty integration.

## Presentation

On macOS, the persistent shortcut footer shows native shortcuts first:

- Global: `⌘S Save`, `⌘F Find`, `⌘P Open`, `⌘B Files`, `⌘Z Undo`, clipboard actions, `^G Push`, and `^Q Quit`.
- Editor: Shift selection, Option word movement and deletion, Command line/document movement, and Command line deletion.

On non-macOS builds, the footer continues to lead with portable `Control` notation. The footer remains three clipped rows; this change does not add vertical UI space or a new help overlay.

## Data flow

1. Terminal lifecycle negotiates enhanced keyboard reporting when supported.
2. Crossterm parses incoming bytes into `KeyEvent` values.
3. The input normalizer converts enhanced or recognized legacy representations into an `AppAction` or `EditorCommand`.
4. The application routes editor commands only while Editing owns focus and no dialog or overlay consumes input.
5. `Editor` applies movement or a single transaction, then existing rendering displays the cursor, selection, dirty state, and history result.
6. On shutdown, the lifecycle restores only terminal modes it successfully enabled.

Dialogs and overlays remain authoritative. Native global shortcuts do not leak through a dialog or overlay unless that surface already handles the corresponding semantic action.

## Error handling and compatibility

- Lack of keyboard enhancement support is a compatibility mode, not an error dialog.
- Legacy shortcuts remain usable in compatibility mode.
- A malformed or unrecognized key sequence is ignored rather than inserted as text.
- Release events remain ignored. Press and repeat events continue to drive normal typing and held-key movement.
- Bracketed paste remains the preferred `Command+V` path and stays one undo transaction.
- Terminal restoration cannot leave keyboard enhancement pushed after Carnet exits normally or through the restoration guard.

## Testing

### Terminal lifecycle

- Supporting terminals push the chosen enhancement flags and pop them once.
- Unsupported or failed capability queries continue without pushing or popping.
- Partial entry failure, explicit restoration, and drop restoration clean up exactly once.

### Key normalization

- Every primary macOS shortcut maps to its expected action.
- Every retained `Control` alias continues to map to the same action.
- `Shift` combines correctly with grapheme, word, line, and document movement.
- Legacy `Alt+B/F` and `Control+U` inputs map only in editor context.
- Unrecognized `Command`, `Option`, or mixed-modifier characters are not inserted.
- Dialogs, search, quick open, Files focus, and Editor focus preserve their routing priority.

### Editor behavior

- Word deletion covers ASCII, Unicode words, composed graphemes, emoji, punctuation, and runs of whitespace.
- Line deletion covers empty lines, line start/end, first/last lines, and files with or without a trailing newline.
- Every new deletion replaces selections and creates exactly one undo entry.
- Undo and redo restore text, cursor, and selection consistently.
- Reversing a Shift selection shrinks through the anchor correctly for every movement family.

### Presentation and integration

- macOS and portable shortcut descriptions contain the correct primary bindings.
- Wide and narrow TUI snapshots reflect the revised footer without layout regressions.
- The Ghostty guide's configuration block is checked against Ghostty's accepted keybinding syntax.
- README and keyboard-reference links resolve.

Implementation finishes with formatting, Clippy across all targets with warnings denied, and the full all-target test suite.

## Acceptance workflow

In configured Ghostty on macOS, a user can open a note and perform this sequence without adding `Control` to any native chord:

1. Hold `Option+Shift` and select several words with the arrow keys.
2. Cut with `Command+X` and paste with `Command+V`.
3. Undo and redo with `Command+Z` and `Command+Shift+Z`.
4. Extend to a line or document boundary with `Command+Shift+Arrow`.
5. Delete by word and to a line boundary with Option/Command deletion.
6. Save with `Command+S`, find with `Command+F`, and quick open with `Command+P`.

The same note remains editable with the existing portable `Control` shortcuts when keyboard enhancement is unavailable.
