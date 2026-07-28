# Manual Git Push Shortcut Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a non-blocking global `Ctrl+G` action that runs ordinary `git push` for the open repository and reports pushed, up-to-date, or failed status.

**Architecture:** Extend the existing reducer/effect/runtime pipeline with a push-specific request identity and typed completion events. Execute `GitRepo::push()` through the existing per-repository serialized background workers, while keeping push state and failures independent from local save/commit state.

**Tech Stack:** Rust 2024, Ratatui/Crossterm, system Git, UUID repository identity, tempfile-backed integration tests, Insta snapshots.

## Global Constraints

- `Ctrl+G` pushes already-committed changes only; it never saves, commits, configures a remote, selects a branch, or creates upstream tracking.
- Git authentication must use the user's existing non-interactive Git/SSH configuration.
- Push is unavailable on repository home, in dialogs/overlays, during a mutation, or while another push is pending.
- Push stays background/non-blocking, serializes with same-repository Git mutations, and ignores stale or cross-repository completion.
- Remote push failure state is independent from local save/commit failure state and contributes to failure exit status until a later successful push.
- The footer keeps all global shortcuts visible around 110 columns and the keyboard/CLI docs distinguish local save/commit from remote push.

---

### Task 1: Git push primitive

**Files:**
- Modify: `src/git.rs`
- Test: `tests/git_workflow.rs`

**Interfaces:**
- Produces: `pub enum PushOutcome { Pushed, UpToDate }`
- Produces: `pub fn GitRepo::push(&self) -> Result<PushOutcome, GitError>`
- Consumes later: the effect executor calls `GitRepo::push()` without extra remote/branch arguments.

- [x] **Step 1: Write failing real-repository tests**

Add local bare-remote helpers and tests equivalent to:

```rust
#[test]
fn push_updates_the_configured_upstream_and_then_reports_up_to_date() {
    let mut repo = TestRepo::initialized_with_bare_upstream();
    repo.commit_file("note.md", "hello\n");

    assert_eq!(repo.git.push().unwrap(), PushOutcome::Pushed);
    assert_eq!(repo.remote_show("HEAD:note.md"), "hello\n");
    assert_eq!(repo.git.push().unwrap(), PushOutcome::UpToDate);
}

#[test]
fn push_without_an_upstream_returns_a_contextual_git_error() {
    let repo = TestRepo::initialized();
    let error = repo.git.push().unwrap_err();
    assert!(error.to_string().contains("push"));
}

#[test]
fn rejected_push_does_not_change_the_local_commit() {
    let mut repo = TestRepo::initialized_with_bare_upstream();
    repo.commit_file("note.md", "local\n");
    let local_head = repo.git_ok(["rev-parse", "HEAD"]).stdout;
    repo.install_rejecting_pre_receive_hook();

    let error = repo.git.push().unwrap_err();

    assert!(error.to_string().contains("push"));
    assert_eq!(repo.git_ok(["rev-parse", "HEAD"]).stdout, local_head);
}
```

Use `git init --bare`, `git remote add origin`, and `git push -u origin HEAD` only in test setup; production code must not establish tracking.

- [x] **Step 2: Run the focused tests and confirm RED**

Run: `cargo test --test git_workflow push_ -- --nocapture`

Expected: compilation fails because `PushOutcome` and `GitRepo::push` do not exist.

- [x] **Step 3: Implement minimal porcelain parsing**

Add:

```rust
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PushOutcome {
    Pushed,
    UpToDate,
}

pub fn push(&self) -> Result<PushOutcome, GitError> {
    let output = self.run_checked(
        "push",
        [OsStr::new("push"), OsStr::new("--porcelain")],
    )?;
    let report = String::from_utf8_lossy(&output.stdout);
    Ok(if report.lines().any(|line| line.starts_with("=")) {
        PushOutcome::UpToDate
    } else {
        PushOutcome::Pushed
    })
}
```

Keep the existing `stdin(Stdio::null())`, directory identity validation, cancellation, and contextual `GitError::CommandFailed` behavior.

- [x] **Step 4: Run focused tests and confirm GREEN**

Run: `cargo test --test git_workflow push_ -- --nocapture`

Expected: both push integration tests pass and the remote contains the committed note.

### Task 2: Reducer state and typed push lifecycle

**Files:**
- Modify: `src/app/state.rs`
- Modify: `src/app/effect.rs`
- Modify: `src/app/update.rs`
- Modify: `src/app/mod.rs`
- Test: `tests/app_transitions.rs`

**Interfaces:**
- Produces: `pub type PushId = u64`
- Produces: `PendingPush { push_id, repository_id, repository_root }`
- Produces: `PushStatus::{Idle, Pushing, Pushed, UpToDate, Failed { message }}`
- Produces: `GlobalAction::Push`, `AppEffect::Push { push_id, repository_id, repository_root, git }`
- Produces: `AppEvent::PushApplied { ... outcome }` and `AppEvent::PushFailed { ... error }`
- Consumes: `PushOutcome` and `GitRepo` from Task 1.

- [x] **Step 1: Write failing reducer tests**

Cover the end-to-end reducer contract with tests equivalent to:

```rust
#[test]
fn push_starts_once_and_blocks_during_mutation_or_existing_push() {
    let (_repo, mut app) = app_with_note(91, "note.md", "hello");
    let first = app.update(AppEvent::Action(AppAction::Global(GlobalAction::Push)));
    assert!(matches!(&first[..], [AppEffect::Push { .. }]));
    assert!(app.pending_push.is_some());
    assert_eq!(app.status.push, PushStatus::Pushing);
    assert!(app.update(AppEvent::Action(AppAction::Global(GlobalAction::Push))).is_empty());
}

#[test]
fn current_push_completion_updates_status_and_stale_completion_is_ignored() {
    let (_repo, mut app) = app_with_note(92, "note.md", "hello");
    let [AppEffect::Push {
        push_id,
        repository_id,
        repository_root,
        ..
    }] = &app.update(AppEvent::Action(AppAction::Global(GlobalAction::Push)))[..]
    else { panic!("expected push effect") };
    let (push_id, repository_id, repository_root) =
        (*push_id, *repository_id, repository_root.clone());

    app.update(AppEvent::PushApplied {
        push_id: push_id + 1,
        repository_id,
        repository_root: repository_root.clone(),
        outcome: PushOutcome::Pushed,
    });
    assert!(app.pending_push.is_some());

    app.update(AppEvent::PushApplied {
        push_id,
        repository_id,
        repository_root,
        outcome: PushOutcome::Pushed,
    });
    assert!(app.pending_push.is_none());
    assert_eq!(app.status.push, PushStatus::Pushed);
}

#[test]
fn failed_push_is_independent_from_commit_failure_and_success_clears_only_push_failure() {
    let (_repo, mut app) = app_with_note(93, "note.md", "hello");
    app.failures.git = Some(UnresolvedFailure {
        kind: FailureKind::Git,
        message: "local commit failed".into(),
    });
    let failed = push_identity(&app.update(AppEvent::Action(AppAction::Global(
        GlobalAction::Push,
    )))[0]);
    app.update(AppEvent::PushFailed {
        push_id: failed.0,
        repository_id: failed.1,
        repository_root: failed.2,
        error: GitError::WorkerPanicked { operation: "push" },
    });
    assert!(app.failures.git.is_some());
    assert!(app.failures.push.is_some());

    let retried = push_identity(&app.update(AppEvent::Action(AppAction::Global(
        GlobalAction::Push,
    )))[0]);
    app.update(AppEvent::PushApplied {
        push_id: retried.0,
        repository_id: retried.1,
        repository_root: retried.2,
        outcome: PushOutcome::UpToDate,
    });
    assert!(app.failures.git.is_some());
    assert!(app.failures.push.is_none());
}
```

Define `push_identity(&AppEffect) -> (PushId, Uuid, PathBuf)` beside the existing mutation identity helpers using an exhaustive `AppEffect::Push` match.

Also assert Home, active dialogs, active overlays, and `pending_mutation` return no push effect; navigation to Home clears `pending_push` so old completion is ignored; unresolved push failure makes quit status `Failure`.

- [x] **Step 2: Run reducer tests and confirm RED**

Run: `cargo test --test app_transitions push_ -- --nocapture`

Expected: compilation fails on the new action, effect, state, and event names.

- [x] **Step 3: Implement reducer lifecycle**

Add independent fields to `App` and `FailureState`:

```rust
pub pending_push: Option<PendingPush>,
pub next_push_id: PushId,
pub push: Option<UnresolvedFailure>,
```

Implement `App::push()` to validate workspace/dialog/overlay/mutation/pending-push preconditions, allocate an ID, record `PushStatus::Pushing`, and return exactly one `AppEffect::Push`. Implement completion helpers that compare all of `push_id`, `repository_id`, and `repository_root` before mutating state. Map outcomes to exact copy `pushed` / `remote up to date`; map errors to `push failed: {error}` and `FailureKind::Git` without touching `failures.git` or `saved_commit_failure`.

- [x] **Step 4: Run reducer tests and confirm GREEN**

Run: `cargo test --test app_transitions push_ -- --nocapture`

Expected: all push reducer tests pass.

### Task 3: Background runtime execution and serialization

**Files:**
- Modify: `src/app/effect.rs`
- Modify: `src/runtime.rs`
- Test: `tests/runtime_workers.rs`

**Interfaces:**
- Consumes: `AppEffect::Push`, `GitRepo::push()`, and push completion events.
- Produces: `RuntimeOperation::Push` and `WorkerKind::Push` for supervised failure/cancellation accounting.

- [x] **Step 1: Write failing runtime tests**

Add tests that dispatch `GlobalAction::Push` against a temporary repository with a bare upstream and assert the runtime eventually emits success. Use the existing blocking worker hook to prove a push and mutation for the same repository do not execute concurrently, while editor/navigation actions can still dispatch while push is blocked.

```rust
let (hook, entered, release) = BlockingHook::new(WorkerKind::Push);
let mut runtime = runtime_with_remote_and_hook(hook);
runtime.dispatch(AppEvent::Action(AppAction::Global(GlobalAction::Push)));
entered.recv_timeout(TEST_TIMEOUT).unwrap();
assert_eq!(runtime.app().status.push, PushStatus::Pushing);
runtime.dispatch(AppEvent::Action(AppAction::Focus(Focus::Tree)));
assert_eq!(workspace(runtime.app()).focus, Focus::Tree);
release.send(()).unwrap();
wait_until(&mut runtime, |app| app.pending_push.is_none());
```

Add a panic/failure assertion that returns a typed push failure carrying the current push/repository identity.

- [x] **Step 2: Run runtime tests and confirm RED**

Run: `cargo test --test runtime_workers push_ -- --nocapture`

Expected: compilation fails because runtime routing has no Push worker/origin.

- [x] **Step 3: Route push through existing effect workers**

Extend `WorkerOrigin::for_effect`, `kind`, `panic_event`, `repository_root`, cancellation selection, effect queue classification, and worker dispatch for Push. In `EffectExecutor::execute`, call:

```rust
self.run_for_root(&repository_root, || match git.push() {
    Ok(outcome) => AppEvent::PushApplied {
        push_id,
        repository_id,
        repository_root,
        outcome,
    },
    Err(error) => AppEvent::PushFailed {
        push_id,
        repository_id,
        repository_root,
        error,
    },
})
```

This reuses the existing per-root mutex and bounded effect worker pool.

- [x] **Step 4: Run runtime tests and confirm GREEN**

Run: `cargo test --test runtime_workers push_ -- --nocapture`

Expected: push completion, non-blocking interaction, panic handling, and same-root serialization pass.

### Task 4: Keymap, footer, status, and documentation

**Files:**
- Modify: `src/ui/keymap.rs`
- Modify: `src/ui/workspace.rs`
- Modify: `tests/ui_keymap.rs`
- Modify: `tests/tui_snapshots.rs`
- Modify: `tests/snapshots/*.snap` through Insta review
- Modify: `docs/keyboard.md`
- Modify: `docs/cli.md`

**Interfaces:**
- Consumes: `GlobalAction::Push` and `PushStatus` from Task 2.
- Produces: visible `^G Push` footer help and exact status copy.

- [x] **Step 1: Write failing keymap and render assertions**

Extend the global shortcut table with:

```rust
('g', KeyModifiers::CONTROL, GlobalAction::Push),
```

Assert the mapping works in Files and Editing but is consumed by dialogs/overlays. Add snapshot coverage for `pushing`, `pushed`, `remote up to date`, and `push failed: ...`, plus a footer assertion containing `^G Push` at approximately 110 columns.

- [x] **Step 2: Run UI tests and confirm RED**

Run: `cargo test --test ui_keymap global_shortcuts -- --nocapture && cargo test --test tui_snapshots push_ -- --nocapture`

Expected: keymap/footer/status assertions fail before presentation code changes.

- [x] **Step 3: Implement presentation and docs**

Map Ctrl+G in `global_action`. Render the push status before generic status text and compact the footer to:

```text
^S Save  ^G Push  ^F Find  ^P Open  ^B Files  ^Z/Y Undo/Redo  ^C/X/V Clipboard  ^A All  ^Q Quit
```

Document that Ctrl+S is local save/commit, Ctrl+G is ordinary remote push, and upstream/authentication must already work through system Git. Document that push failure leaves local commits untouched and is retried with Ctrl+G.

- [x] **Step 4: Run UI/docs tests and accept intended snapshots**

Run: `cargo test --test ui_keymap -- --nocapture`

Run: `INSTA_UPDATE=always cargo test --test tui_snapshots`

Run: `cargo test --test docs_drift`

Expected: keymaps, intended snapshots, and documentation drift tests pass.

### Task 5: Full verification and local installation

**Files:**
- Verify all modified files from Tasks 1-4.
- Build/install: `target/release/carnet` and `~/.cargo/bin/carnet`.

**Interfaces:**
- Consumes the complete feature.
- Produces a verified release binary matching the current working tree.

- [x] **Step 1: Format and run warnings-as-errors lint**

Run: `cargo fmt --all -- --check`

Run: `cargo clippy --all-targets --all-features -- -D warnings`

Expected: both commands exit 0.

- [x] **Step 2: Run the full test suite**

Run: `cargo test --all-targets --all-features`

Expected: every unit, integration, snapshot, and doc test passes.

- [x] **Step 3: Inspect the final diff**

Run: `git diff --check && git status --short && git diff --stat`

Expected: no whitespace errors, only planned source/test/docs changes, and no unexpected generated files.

- [x] **Step 4: Build and install the local binary**

Run: `cargo install --path . --locked --force`

Run: `~/.cargo/bin/carnet --version`

Expected: install succeeds and reports the repository package version.
