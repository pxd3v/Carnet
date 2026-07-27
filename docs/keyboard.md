# Keyboard reference

Carnet is fully usable from the keyboard. Mouse input is not required.

## Global shortcuts

These shortcuts work from the applicable repository workspace unless a dialog or search surface is consuming input.

| Key | Action |
| --- | --- |
| `Ctrl+S` | Save the current note, or retry a failed Git commit without rewriting the file |
| `Ctrl+F` | Open literal find in the current note |
| `Ctrl+P` | Open quick open for text files in the current repository |
| `Ctrl+B` | Focus/show the tree; from the tree, hide it and return to the editor |
| `Ctrl+Z` | Undo |
| `Ctrl+Shift+Z` or `Ctrl+Y` | Redo |
| `Ctrl+C` | Copy the selection |
| `Ctrl+X` | Cut the selection |
| `Ctrl+V` | Paste from the system clipboard or process-local fallback |
| `Ctrl+A` | Select all note text |
| `Ctrl+Q` | Quit; a dirty note opens the dirty-navigation dialog |

Terminal bracketed-paste input is inserted as one editor transaction, so one undo removes the entire paste.

## Repository home

| Key | Action |
| --- | --- |
| `Up` / `Down` | Change the selected registration |
| `Enter` | Open the selected available repository |
| `c` | Create a repository with `git init`, then register it |
| `a` | Register an existing Git work-tree root |
| `Shift+R` | Rename the selected registration |
| `d` | Set the selected available repository as default |
| `u` | Unregister the selection without deleting anything from disk |

When a CLI note path is waiting for a default, choosing the default resumes that exact path.

## Tree

Use `Ctrl+B` to enter tree focus.

| Key | Action |
| --- | --- |
| `Up` / `Down` | Move through visible entries |
| `Right` | Expand a selected directory |
| `Left` | Collapse a directory or move to its visible parent |
| `Enter` | Open a text file, or toggle a directory |
| `n` | Create a file |
| `Shift+N` | Create a folder |
| `r` | Rename the selected file or folder |
| `m` | Move the selected file or folder |
| `Delete` | Open delete confirmation |
| `Escape` | Return to the editor; also closes a narrow-width tree overlay |

Binary/non-UTF-8 files and symlinks can appear in the tree but cannot be opened as notes.

## Editor

| Key | Action |
| --- | --- |
| Arrow keys | Move by grapheme horizontally or by visual column vertically |
| `Shift` + arrow key | Extend the selection |
| `Home` / `End` | Move to the start/end of the current line; hold `Shift` to select |
| Text input | Replace the selection or insert at the cursor |
| `Enter` | Insert a newline |
| `Backspace` / `Delete` | Delete before/after the cursor, or delete the selection |
| `Tab` | Indent the selected lines or current line |
| `Shift+Tab` | Outdent the selected lines or current line |

Save, find, quick open, sidebar, undo/redo, clipboard, select-all, and quit use the global shortcuts above.

## Find and quick open

Find (`Ctrl+F`):

| Key | Action |
| --- | --- |
| Text input / `Backspace` | Edit the literal query |
| `Enter` | Select the next match, wrapping at the end |
| `Shift+Enter` | Select the previous match, wrapping at the beginning |
| `Escape` | Close find |

Quick open (`Ctrl+P`):

| Key | Action |
| --- | --- |
| Text input / `Backspace` | Filter repository-relative text-file paths |
| `Up` / `Down` | Change the selected match |
| `Enter` | Open the selected match |
| `Escape` | Close quick open |

## Dialogs

| Dialog | Keys |
| --- | --- |
| Dirty navigation | `s` save and continue; `d` discard and continue; `c` or `Escape` cancel |
| External conflict | `r` reload disk content; `o` overwrite disk content; `c` or `Escape` cancel |
| Saved, commit failed | `r` or `s` retry the commit; `Escape` dismiss |
| Delete confirmation | `y` or `Enter` delete; `n` or `Escape` cancel |
| Set-default / unregister confirmation | `y` or `Enter` confirm; `n` or `Escape` cancel |
| Repository create/register form | Type and `Backspace` edit; `Tab` changes field; `Enter` submits; `Escape` cancels |
| Rename-registration form | Type and `Backspace` edit; `Enter` submits; `Escape` cancels |
| File create/rename/move form | Type and `Backspace` edit; `Enter` submits; `Escape` cancels |
| Runtime/write failure | `Enter` or `Escape` dismisses the message; unresolved failures still determine exit status |
