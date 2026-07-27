use std::{fs, path::PathBuf, process::Command};

use carnet::{
    app::{
        App, AppAction, AppEffect, AppEvent, EffectExecutor, ExternalConflict, FailureKind,
        HomeAction, NavigationAction, RequestId, RuntimeError, RuntimeOperation,
    },
    catalog::RepoEntry,
    git::{CommitIntent, CommitOutcome, GitRepo},
    workspace::{FileOperation, FileOutcome, Workspace},
};
use tempfile::{TempDir, tempdir};
use uuid::Uuid;

#[test]
fn executor_opens_a_workspace_and_loads_the_requested_note() {
    let repo = TestRepo::new(1);
    fs::write(repo.root().join("note.md"), "hello").unwrap();

    let event = EffectExecutor::default()
        .execute(AppEffect::OpenWorkspace {
            request_id: RequestId::new(1),
            repository: repo.entry.clone(),
            note: Some(PathBuf::from("note.md")),
        })
        .unwrap();

    match event {
        AppEvent::WorkspaceOpened {
            repository_id,
            workspace,
            tree,
            note: Some(note),
            ..
        } => {
            assert_eq!(repository_id, repo.entry.id);
            assert_eq!(workspace.root(), repo.root());
            assert_eq!(tree.len(), 1);
            assert_eq!(note.text(), "hello");
        }
        other => panic!("unexpected event: {other:?}"),
    }
}

#[test]
fn executor_applies_and_commits_a_save_then_maps_external_conflicts() {
    let repo = TestRepo::new(2);
    repo.configure_identity();
    fs::write(repo.root().join("note.md"), "before").unwrap();
    repo.git
        .commit_all(CommitIntent::Create(PathBuf::from("note.md")))
        .unwrap();
    let workspace = Workspace::open(repo.entry.clone()).unwrap();
    let note = workspace
        .load_note(
            &workspace
                .resolve_note(PathBuf::from("note.md").as_path())
                .unwrap(),
        )
        .unwrap();

    let event = EffectExecutor::default()
        .execute(AppEffect::ApplyAndCommit {
            repository_id: repo.entry.id,
            workspace: workspace.clone(),
            git: repo.git.clone(),
            operation: Box::new(FileOperation::Save {
                note,
                content: "after".into(),
                overwrite: false,
            }),
            intent: CommitIntent::Update(PathBuf::from("note.md")),
        })
        .unwrap();

    assert!(matches!(
        event,
        AppEvent::MutationApplied {
            file: FileOutcome::Saved(_),
            commit: CommitOutcome::Committed { .. },
            ..
        }
    ));
    assert_eq!(
        fs::read_to_string(repo.root().join("note.md")).unwrap(),
        "after"
    );

    let stale = workspace
        .load_note(
            &workspace
                .resolve_note(PathBuf::from("note.md").as_path())
                .unwrap(),
        )
        .unwrap();
    fs::write(repo.root().join("note.md"), "external").unwrap();
    let conflict = EffectExecutor::default()
        .execute(AppEffect::ApplyAndCommit {
            repository_id: repo.entry.id,
            workspace: workspace.clone(),
            git: repo.git.clone(),
            operation: Box::new(FileOperation::Save {
                note: stale,
                content: "mine".into(),
                overwrite: false,
            }),
            intent: CommitIntent::Update(PathBuf::from("note.md")),
        })
        .unwrap();
    assert!(matches!(
        conflict,
        AppEvent::MutationConflict {
            conflict: ExternalConflict::Modified { .. },
            ..
        }
    ));
    assert_eq!(
        fs::read_to_string(repo.root().join("note.md")).unwrap(),
        "external"
    );

    let deleted = workspace
        .load_note(
            &workspace
                .resolve_note(PathBuf::from("note.md").as_path())
                .unwrap(),
        )
        .unwrap();
    fs::remove_file(repo.root().join("note.md")).unwrap();
    let conflict = EffectExecutor::default()
        .execute(AppEffect::ApplyAndCommit {
            repository_id: repo.entry.id,
            workspace,
            git: repo.git.clone(),
            operation: Box::new(FileOperation::Save {
                note: deleted,
                content: "mine".into(),
                overwrite: false,
            }),
            intent: CommitIntent::Update(PathBuf::from("note.md")),
        })
        .unwrap();
    assert!(matches!(
        conflict,
        AppEvent::MutationConflict {
            conflict: ExternalConflict::Deleted { .. },
            ..
        }
    ));
}

#[test]
fn executor_preserves_a_saved_file_across_commit_failure_and_retries_only_git() {
    let repo = TestRepo::new(3);
    repo.configure_identity();
    fs::write(repo.root().join("note.md"), "before").unwrap();
    repo.git
        .commit_all(CommitIntent::Create(PathBuf::from("note.md")))
        .unwrap();
    let workspace = Workspace::open(repo.entry.clone()).unwrap();
    let note = workspace
        .load_note(
            &workspace
                .resolve_note(PathBuf::from("note.md").as_path())
                .unwrap(),
        )
        .unwrap();
    repo.git_ok(["config", "user.name", ""]);
    repo.git_ok(["config", "user.email", ""]);
    let intent = CommitIntent::Update(PathBuf::from("note.md"));

    let event = EffectExecutor::default()
        .execute(AppEffect::ApplyAndCommit {
            repository_id: repo.entry.id,
            workspace,
            git: repo.git.clone(),
            operation: Box::new(FileOperation::Save {
                note,
                content: "saved".into(),
                overwrite: false,
            }),
            intent: intent.clone(),
        })
        .unwrap();

    assert!(matches!(event, AppEvent::MutationSavedCommitFailed { .. }));
    assert_eq!(
        fs::read_to_string(repo.root().join("note.md")).unwrap(),
        "saved"
    );

    repo.configure_identity();
    let retry = EffectExecutor::default()
        .execute(AppEffect::RetryCommit {
            repository_id: repo.entry.id,
            git: repo.git.clone(),
            intent,
        })
        .unwrap();
    assert!(matches!(retry, AppEvent::CommitRetryApplied { .. }));
    assert_eq!(
        fs::read_to_string(repo.root().join("note.md")).unwrap(),
        "saved"
    );
}

#[test]
fn load_failures_return_typed_runtime_events_and_leave_app_state_retryable() {
    let repo = TestRepo::new(4);
    fs::write(repo.root().join("valid.md"), "valid").unwrap();
    let workspace = Workspace::open(repo.entry.clone()).unwrap();
    let mut app = App::home(vec![repo.entry.clone()], Some(repo.entry.id), None);
    app.update(AppEvent::Action(AppAction::Home(HomeAction::OpenSelected)));
    let request_id = app.pending_request.as_ref().unwrap().request_id();
    app.update(AppEvent::WorkspaceOpened {
        request_id,
        repository_id: repo.entry.id,
        workspace: workspace.clone(),
        git: repo.git.clone(),
        tree: Vec::new(),
        note: None,
    });
    app.update(AppEvent::ClipboardWritten(Err(
        carnet::editor::ClipboardError::Unavailable,
    )));
    let effects = app.update(AppEvent::Action(AppAction::Navigate(
        NavigationAction::Note(PathBuf::from("/outside.md")),
    )));

    let event = EffectExecutor::default()
        .execute(effects.into_iter().next().unwrap())
        .unwrap();
    assert!(matches!(
        &event,
        AppEvent::RuntimeFailed {
            repository_id,
            operation: RuntimeOperation::LoadNote,
            error: RuntimeError::Path(_),
            ..
        } if *repository_id == repo.entry.id
    ));

    app.update(event);
    assert_eq!(app.pending_request, None);
    assert_eq!(
        app.failures.runtime.first().map(|_| FailureKind::Runtime),
        Some(FailureKind::Runtime),
    );
    assert!(app.failures.clipboard.is_some());

    let valid = app
        .update(AppEvent::Action(AppAction::Navigate(
            NavigationAction::Note(PathBuf::from("valid.md")),
        )))
        .pop()
        .unwrap();
    app.update(EffectExecutor::default().execute(valid).unwrap());

    assert!(app.failures.runtime.is_empty());
    assert!(app.failures.clipboard.is_some());
}

#[test]
fn executor_returns_outer_runtime_effects_intact() {
    let error = EffectExecutor::default()
        .execute(AppEffect::ReadClipboard)
        .unwrap_err();

    assert!(matches!(error.into_effect(), AppEffect::ReadClipboard));
}

#[test]
fn out_of_order_open_load_and_failure_results_are_ignored_by_request_id() {
    let repo_a = TestRepo::new(50);
    let repo_b = TestRepo::new(51);
    fs::write(repo_b.root().join("current.md"), "current").unwrap();
    fs::write(repo_b.root().join("one.md"), "one").unwrap();
    let mut app = App::home(
        vec![repo_a.entry.clone(), repo_b.entry.clone()],
        Some(repo_a.entry.id),
        None,
    );
    let executor = EffectExecutor::default();

    let open_a = app
        .update(AppEvent::Action(AppAction::Navigate(
            NavigationAction::Repository {
                repository: repo_a.entry.clone(),
                note: None,
            },
        )))
        .pop()
        .unwrap();
    let open_b = app
        .update(AppEvent::Action(AppAction::Navigate(
            NavigationAction::Repository {
                repository: repo_b.entry.clone(),
                note: None,
            },
        )))
        .pop()
        .unwrap();
    assert!(request_id(&open_a) < request_id(&open_b));
    let current_open_id = app.pending_request.as_ref().unwrap().request_id();

    app.update(executor.execute(open_a).unwrap());
    assert!(matches!(app.screen, carnet::app::Screen::Home));
    assert_eq!(
        app.pending_request.as_ref().unwrap().request_id(),
        current_open_id
    );
    app.update(executor.execute(open_b).unwrap());
    let carnet::app::Screen::Workspace(workspace) = &app.screen else {
        panic!("expected current workspace");
    };
    assert_eq!(workspace.repository.id, repo_b.entry.id);

    let stale_success = app
        .update(AppEvent::Action(AppAction::Navigate(
            NavigationAction::Note(PathBuf::from("one.md")),
        )))
        .pop()
        .unwrap();
    let current_success = app
        .update(AppEvent::Action(AppAction::Navigate(
            NavigationAction::Note(PathBuf::from("current.md")),
        )))
        .pop()
        .unwrap();
    assert!(request_id(&stale_success) < request_id(&current_success));
    let current_success_id = app.pending_request.as_ref().unwrap().request_id();
    app.update(executor.execute(stale_success).unwrap());
    assert_eq!(
        app.pending_request.as_ref().unwrap().request_id(),
        current_success_id
    );
    app.update(executor.execute(current_success).unwrap());
    let carnet::app::Screen::Workspace(workspace) = &app.screen else {
        panic!("expected current workspace");
    };
    assert_eq!(
        workspace.current_note.as_deref(),
        Some(PathBuf::from("current.md").as_path())
    );

    let stale_load = app
        .update(AppEvent::Action(AppAction::Navigate(
            NavigationAction::Note(PathBuf::from("/outside.md")),
        )))
        .pop()
        .unwrap();
    let current_load = app
        .update(AppEvent::Action(AppAction::Navigate(
            NavigationAction::Note(PathBuf::from("current.md")),
        )))
        .pop()
        .unwrap();
    assert!(request_id(&stale_load) < request_id(&current_load));
    let current_load_id = app.pending_request.as_ref().unwrap().request_id();

    app.update(executor.execute(stale_load).unwrap());
    assert_eq!(
        app.pending_request.as_ref().unwrap().request_id(),
        current_load_id
    );
    assert!(app.failures.runtime.is_empty());
    app.update(executor.execute(current_load).unwrap());

    assert_eq!(app.pending_request, None);
    let carnet::app::Screen::Workspace(workspace) = &app.screen else {
        panic!("expected current workspace");
    };
    assert_eq!(
        workspace.current_note.as_deref(),
        Some(PathBuf::from("current.md").as_path())
    );
}

#[test]
fn newer_load_supersedes_an_older_workspace_open() {
    let repo_a = TestRepo::new(52);
    let repo_b = TestRepo::new(53);
    fs::write(repo_a.root().join("newest.md"), "newest").unwrap();
    let executor = EffectExecutor::default();
    let mut app = App::home(
        vec![repo_a.entry.clone(), repo_b.entry.clone()],
        Some(repo_a.entry.id),
        None,
    );

    let initial_open = app
        .update(AppEvent::Action(AppAction::Navigate(
            NavigationAction::Repository {
                repository: repo_a.entry.clone(),
                note: None,
            },
        )))
        .pop()
        .unwrap();
    app.update(executor.execute(initial_open).unwrap());

    let stale_open = app
        .update(AppEvent::Action(AppAction::Navigate(
            NavigationAction::Repository {
                repository: repo_b.entry.clone(),
                note: None,
            },
        )))
        .pop()
        .unwrap();
    let newest_load = app
        .update(AppEvent::Action(AppAction::Navigate(
            NavigationAction::Note(PathBuf::from("newest.md")),
        )))
        .pop()
        .unwrap();
    assert!(request_id(&stale_open) < request_id(&newest_load));

    app.update(executor.execute(stale_open).unwrap());
    let carnet::app::Screen::Workspace(workspace) = &app.screen else {
        panic!("expected current workspace");
    };
    assert_eq!(workspace.repository.id, repo_a.entry.id);

    app.update(executor.execute(newest_load).unwrap());
    let carnet::app::Screen::Workspace(workspace) = &app.screen else {
        panic!("expected current workspace");
    };
    assert_eq!(workspace.repository.id, repo_a.entry.id);
    assert_eq!(
        workspace.current_note.as_deref(),
        Some(PathBuf::from("newest.md").as_path())
    );
}

#[test]
fn newer_workspace_open_supersedes_an_older_failing_load() {
    let repo_a = TestRepo::new(54);
    let repo_b = TestRepo::new(55);
    let executor = EffectExecutor::default();
    let mut app = App::home(
        vec![repo_a.entry.clone(), repo_b.entry.clone()],
        Some(repo_a.entry.id),
        None,
    );

    let initial_open = app
        .update(AppEvent::Action(AppAction::Navigate(
            NavigationAction::Repository {
                repository: repo_a.entry.clone(),
                note: None,
            },
        )))
        .pop()
        .unwrap();
    app.update(executor.execute(initial_open).unwrap());

    let stale_load = app
        .update(AppEvent::Action(AppAction::Navigate(
            NavigationAction::Note(PathBuf::from("/outside.md")),
        )))
        .pop()
        .unwrap();
    let newest_open = app
        .update(AppEvent::Action(AppAction::Navigate(
            NavigationAction::Repository {
                repository: repo_b.entry.clone(),
                note: None,
            },
        )))
        .pop()
        .unwrap();
    assert!(request_id(&stale_load) < request_id(&newest_open));
    let newest_request_id = request_id(&newest_open);

    app.update(executor.execute(stale_load).unwrap());
    assert!(app.failures.runtime.is_empty());
    assert_eq!(
        app.pending_request.as_ref().unwrap().request_id().get(),
        newest_request_id
    );
    let carnet::app::Screen::Workspace(workspace) = &app.screen else {
        panic!("expected current workspace");
    };
    assert_eq!(workspace.repository.id, repo_a.entry.id);

    app.update(executor.execute(newest_open).unwrap());
    let carnet::app::Screen::Workspace(workspace) = &app.screen else {
        panic!("expected newest workspace");
    };
    assert_eq!(workspace.repository.id, repo_b.entry.id);
    assert_eq!(app.pending_request, None);
}

#[cfg(unix)]
#[test]
fn open_and_load_share_mutation_lock_per_root_without_blocking_other_roots() {
    use std::{
        os::unix::fs::PermissionsExt,
        sync::mpsc::{self, RecvTimeoutError},
        thread,
        time::{Duration, Instant},
    };

    let repo_a = TestRepo::new(60);
    repo_a.configure_identity();
    fs::write(repo_a.root().join("note.md"), "before").unwrap();
    repo_a
        .git
        .commit_all(CommitIntent::Create(PathBuf::from("note.md")))
        .unwrap();
    let workspace_a = Workspace::open(repo_a.entry.clone()).unwrap();
    let note_a = workspace_a
        .load_note(
            &workspace_a
                .resolve_note(PathBuf::from("note.md").as_path())
                .unwrap(),
        )
        .unwrap();
    let hook = repo_a.root().join(".git/hooks/pre-commit");
    fs::write(
        &hook,
        "#!/bin/sh\ntouch hook-entered\nwhile [ ! -f hook-release ]; do sleep 0.01; done\n",
    )
    .unwrap();
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();

    let executor = EffectExecutor::default();
    let same_load_workspace = Workspace::open(repo_a.entry.clone()).unwrap();
    let mutation_executor = executor.clone();
    let mutation_repository_id = workspace_a.repo().id;
    let mutation_git = repo_a.git.clone();
    let mutation = thread::spawn(move || {
        mutation_executor.execute(AppEffect::ApplyAndCommit {
            repository_id: mutation_repository_id,
            workspace: workspace_a,
            git: mutation_git,
            operation: Box::new(FileOperation::Save {
                note: note_a,
                content: "after".into(),
                overwrite: false,
            }),
            intent: CommitIntent::Update(PathBuf::from("note.md")),
        })
    });
    wait_for_path(
        &repo_a.root().join("hook-entered"),
        Instant::now() + Duration::from_secs(2),
    );

    let (same_open_tx, same_open_rx) = mpsc::channel();
    let same_open_executor = executor.clone();
    let same_open_entry = repo_a.entry.clone();
    let same_open = thread::spawn(move || {
        same_open_tx
            .send(same_open_executor.execute(AppEffect::OpenWorkspace {
                request_id: RequestId::new(1),
                repository: same_open_entry,
                note: None,
            }))
            .unwrap();
    });
    let (same_load_tx, same_load_rx) = mpsc::channel();
    let same_load_executor = executor.clone();
    let same_load = thread::spawn(move || {
        same_load_tx
            .send(same_load_executor.execute(AppEffect::LoadNote {
                request_id: RequestId::new(2),
                repository_id: same_load_workspace.repo().id,
                workspace: same_load_workspace,
                path: PathBuf::from("note.md"),
            }))
            .unwrap();
    });
    assert!(matches!(
        same_open_rx.recv_timeout(Duration::from_millis(100)),
        Err(RecvTimeoutError::Timeout)
    ));
    assert!(matches!(
        same_load_rx.recv_timeout(Duration::from_millis(100)),
        Err(RecvTimeoutError::Timeout)
    ));

    let repo_b = TestRepo::new(61);
    fs::write(repo_b.root().join("note.md"), "other").unwrap();
    let workspace_b = Workspace::open(repo_b.entry.clone()).unwrap();
    let (other_tx, other_rx) = mpsc::channel();
    let other_open_executor = executor.clone();
    let other_load_executor = executor.clone();
    let other_entry = repo_b.entry.clone();
    let other_workspace = workspace_b.clone();
    let other = thread::spawn(move || {
        let opened = other_open_executor.execute(AppEffect::OpenWorkspace {
            request_id: RequestId::new(3),
            repository: other_entry,
            note: None,
        });
        let loaded = other_load_executor.execute(AppEffect::LoadNote {
            request_id: RequestId::new(4),
            repository_id: other_workspace.repo().id,
            workspace: other_workspace,
            path: PathBuf::from("note.md"),
        });
        other_tx.send((opened, loaded)).unwrap();
    });
    let (opened, loaded) = other_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    assert!(matches!(opened.unwrap(), AppEvent::WorkspaceOpened { .. }));
    assert!(matches!(loaded.unwrap(), AppEvent::NoteLoaded { .. }));

    fs::write(repo_a.root().join("hook-release"), "release").unwrap();
    same_open_rx
        .recv_timeout(Duration::from_secs(2))
        .unwrap()
        .unwrap();
    same_load_rx
        .recv_timeout(Duration::from_secs(2))
        .unwrap()
        .unwrap();
    mutation.join().unwrap().unwrap();
    same_open.join().unwrap();
    same_load.join().unwrap();
    other.join().unwrap();

    fn wait_for_path(path: &std::path::Path, deadline: Instant) {
        while !path.exists() {
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {}",
                path.display()
            );
            thread::sleep(Duration::from_millis(5));
        }
    }
}

struct TestRepo {
    _sandbox: TempDir,
    entry: RepoEntry,
    git: GitRepo,
}

impl TestRepo {
    fn new(id: u128) -> Self {
        let sandbox = tempdir().unwrap();
        let root = fs::canonicalize(sandbox.path()).unwrap();
        let entry = RepoEntry {
            id: Uuid::from_u128(id),
            name: format!("repo-{id}"),
            path: root.clone(),
        };
        let git = GitRepo::initialize(&root).unwrap();
        Self {
            _sandbox: sandbox,
            entry,
            git,
        }
    }

    fn root(&self) -> &std::path::Path {
        &self.entry.path
    }

    fn configure_identity(&self) {
        self.git_ok(["config", "user.name", "Carnet Test"]);
        self.git_ok(["config", "user.email", "carnet@example.test"]);
    }

    fn git_ok<const N: usize>(&self, args: [&str; N]) {
        let output = Command::new("git")
            .args(args)
            .current_dir(self.root())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

fn request_id(effect: &AppEffect) -> u64 {
    match effect {
        AppEffect::OpenWorkspace { request_id, .. } | AppEffect::LoadNote { request_id, .. } => {
            request_id.get()
        }
        other => panic!("effect has no request ID: {other:?}"),
    }
}
