# Manual Git Push Shortcut

## Goal

Add one explicit global shortcut that pushes already-committed Carnet repository changes to its configured upstream without coupling remote synchronization to saving or editing.

## User interaction

`Ctrl+G` runs ordinary `git push` for the currently open repository. It works from Files or Editing whenever no dialog or overlay is consuming input.

The shortcut pushes local commits only. It does not save dirty editor content, create a commit, configure a remote, select a branch, or establish upstream tracking. Users keep using `Ctrl+S` for local save/commit and `Ctrl+G` for remote push.

While the operation runs, the status line shows `pushing`. Completion shows:

- `pushed` when Git reports a successful update;
- `remote up to date` when there was nothing to send;
- `push failed: <Git error>` when Git exits unsuccessfully.

A failed push leaves local files and commits untouched. Pressing `Ctrl+G` retries. Carnet uses the repository's existing Git credential and SSH-agent configuration; the child process has no interactive stdin, so missing credentials fail visibly instead of hanging the terminal UI.

`Ctrl+G` is inert on repository home, while a save/file mutation is pending, or while another push is pending. Editing and browsing remain available during the background push. Navigating to another repository invalidates the old push result so it cannot update the new repository's status.

## Architecture

`GlobalAction` gains `Push`. The app maps it to an `AppEffect::Push` containing a unique request ID, repository identity, repository root, and `GitRepo`. App state tracks one `PendingPush` independently from note loads and filesystem mutations.

The existing background runtime executes the push through `GitRepo`, using the same directory-identity validation, cancellation, process-group supervision, and per-repository serialization as other Git operations. `GitRepo::push()` executes `git push --porcelain` and maps success to `PushOutcome::Pushed` or `PushOutcome::UpToDate`. It does not add arguments that change remote configuration.

Completion and failure return typed app events with the request and repository identity. The update layer accepts only the current pending push, clears it exactly once, and updates status/failure state. Stale, duplicate, cross-repository, and post-navigation results are ignored.

Remote push failure is stored in a dedicated push-failure slot and contributes to failure exit status until a later successful push clears it. Local save/commit failure and retry state remains independent and is never cleared by pushing.

## Presentation and documentation

The Global footer row adds `^G Push`. To keep every global shortcut visible around 110 columns, related keys may be compacted, for example `^Z/Y Undo/Redo` and `^C/X/V Clipboard`, without removing any shortcut.

The keyboard and CLI documentation will explicitly distinguish local `Ctrl+S` save/commit from remote `Ctrl+G` push, and state that upstream tracking and authentication must already work with ordinary `git push`.

## Testing

Tests use real temporary repositories and local bare remotes to prove:

- `Ctrl+G` maps to the global Push action from Files and Editing;
- repository home, dialogs, and overlays do not start a push;
- the app emits one push effect, rejects duplicates and mutation overlap, and ignores stale/cross-repository results;
- `GitRepo::push()` updates an upstream bare remote and reports an already-current remote;
- missing upstream and rejected pushes return contextual Git errors without changing local commits;
- runtime execution returns typed success/failure events and remains serialized with same-repository mutations;
- status, failure recovery, footer content, docs, and exit status reflect pushing, success, up-to-date, failure, and retry.

Implementation follows red-green-refactor and finishes with formatting, Clippy with warnings denied, and the full all-target/all-feature test suite.
