use std::{fs, path::PathBuf, process::Command};

use carnet::{
    app::{
        App, AppAction, AppEffect, AppEvent, EffectExecutor, ExternalConflict, FailureKind,
        NavigationAction, RuntimeError, RuntimeOperation,
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
    let workspace = Workspace::open(repo.entry.clone()).unwrap();
    let mut app = App::home(vec![repo.entry.clone()], Some(repo.entry.id), None);
    app.update(AppEvent::WorkspaceOpened {
        repository_id: repo.entry.id,
        workspace: workspace.clone(),
        git: repo.git.clone(),
        tree: Vec::new(),
        note: None,
    });
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
        } if *repository_id == repo.entry.id
    ));

    app.update(event);
    assert_eq!(app.pending_load, None);
    assert_eq!(
        app.failure.as_ref().map(|failure| failure.kind),
        Some(FailureKind::Runtime)
    );
}

#[test]
fn executor_returns_outer_runtime_effects_intact() {
    let error = EffectExecutor::default()
        .execute(AppEffect::ReadClipboard)
        .unwrap_err();

    assert!(matches!(error.into_effect(), AppEffect::ReadClipboard));
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
