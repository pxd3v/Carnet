# Keyboard reference

Carnet is fully usable from the keyboard. Mouse input is not required.

## Global shortcuts

These shortcuts work from the applicable repository workspace unless a dialog or search surface is consuming input.

| Key | Action |
| --- | --- |
| `Command+S` or `Ctrl+S` | Save the current note and create a local commit, or retry a failed local commit without rewriting the file |
| `Ctrl+G` | Push already-committed changes with ordinary `git push`; retry a failed push |
| `Command+F` or `Ctrl+F` | Open literal find in the current note |
| `Command+P` or `Ctrl+P` | Open quick open for text files in the current repository |
| `Command+B` or `Ctrl+B` | Safely return to Files from Editing; from Files, hide it and edit the current preview |
| `Command+Z` or `Ctrl+Z` | Undo |
| `Command+Shift+Z`, `Ctrl+Shift+Z`, or `Ctrl+Y` | Redo |
| `Command+C` or `Ctrl+C` | Copy the selection |
| `Command+X` or `Ctrl+X` | Cut the selection |
| `Command+V` or `Ctrl+V` | Paste from the system clipboard or process-local fallback |
| `Command+A` or `Ctrl+A` | Select all note text |
| `Ctrl+Q` | Quit; a dirty note opens the dirty-navigation dialog |

Terminal bracketed-paste input is inserted as one editor transaction, so one undo removes the entire paste.

Ghostty owns several macOS shortcuts by default. Follow [Ghostty setup for native macOS editing](ghostty.md) once to forward them to Carnet. The `Ctrl` aliases remain available in every supported terminal.

`Ctrl+G` never saves dirty editor content, creates a commit, chooses a branch, or configures a remote. The current branch must already have upstream tracking, and authentication must already work non-interactively through the system Git/SSH configuration. A failed push leaves local files and commits untouched.

## Repository home

| Key | Action |
| --- | --- |
| `Up` / `Down` | Change the selected registration |
| `Enter` | Open the selected available repository; when a pending CLI note needs a default, persist this selection as default first and then resume the note |
| `c` | Create a repository with `git init`, then register it |
| `a` | Register an existing Git work-tree root |
| `Shift+R` | Rename the selected registration |
| `d` | Set the selected available repository as default |
| `u` | Unregister the selection without deleting anything from disk |

When a CLI note path is waiting for a default, choosing the default resumes that exact path.

## Files and Preview

Files is a current-folder browser. Selecting a text file previews it on the right without enabling edits.

| Key | Action |
| --- | --- |
| `Up` / `Down` | Select an entry and preview it when it is a text file |
| `Right` or `Enter` on a folder | Enter the folder and show only its direct children |
| `Left` | Return to the parent folder |
| `Enter` on a text file | Move to Editing for the selected preview |
| `n` | Create a file |
| `Shift+N` | Create a folder |
| `r` | Rename the selected file or folder |
| `m` | Move the selected file or folder |
| `Delete` | Open delete confirmation |

Selecting a folder, binary/non-UTF-8 file, or symlink clears the preview. Binary files and symlinks cannot be edited as notes.

## Editor

| Key | Action |
| --- | --- |
| Arrow keys | Move by grapheme horizontally or by visual column vertically |
| `Shift` + arrow key | Extend the selection |
| `Home` / `End` | Move to the start/end of the current line; hold `Shift` to select |
| `Option+Left` / `Option+Right` | Move to the previous word start / next word end |
| `Command+Left` / `Command+Right` | Move to the start/end of the current line |
| `Command+Up` / `Command+Down` | Move to the start/end of the document |
| `Option+Backspace` / `Option+Delete` | Delete through the previous word start / next word end |
| `Command+Backspace` / `Command+Delete` | Delete to the start/end of the current line without joining lines |
| Text input | Replace the selection or insert at the cursor |
| `Enter` or `Shift+Enter` | Insert a newline |
| `Backspace` / `Delete` | Delete before/after the cursor, or delete the selection |
| `Tab` | Indent the selected lines or current line |
| `Shift+Tab` | Outdent the selected lines or current line |
| `Escape` | Return to Files; prompts Save / Discard / Cancel when modified |

Adding Shift to any arrow, Option movement, or Command movement extends the selection from a fixed anchor. Reversing direction shrinks through that anchor. Carnet receives Option as Alt and Command as Super; your terminal must forward those combinations instead of reserving them.

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
