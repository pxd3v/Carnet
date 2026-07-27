use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use carnet::git::{
    CommitIntent, CommitOutcome, GitRepo, MutationCommitError, MutationCommitOutcome,
    apply_and_commit,
};
use carnet::{
    catalog::RepoEntry,
    workspace::{FileOperation, Workspace},
};
use tempfile::{TempDir, tempdir};
use uuid::Uuid;

#[test]
fn initial_commit_tracks_all_content_with_the_create_message() {
    let repo = TestRepo::initialized();
    fs::write(repo.path().join("note.md"), "hello\n").unwrap();

    let outcome = repo
        .git
        .commit_all(CommitIntent::Create(PathBuf::from("note.md")))
        .unwrap();

    assert!(matches!(outcome, CommitOutcome::Committed { .. }));
    assert_eq!(repo.subjects(), vec!["carnet: create note.md"]);
    assert_eq!(repo.show("HEAD:note.md"), "hello\n");
}

#[test]
fn no_changes_and_empty_directories_do_not_create_commits() {
    let repo = TestRepo::initialized();
    fs::create_dir(repo.path().join("empty-folder")).unwrap();

    let outcome = repo
        .git
        .commit_all(CommitIntent::Create(PathBuf::from("empty-folder")))
        .unwrap();

    assert_eq!(outcome, CommitOutcome::NoChanges);
    assert!(
        !repo
            .git_command(["rev-parse", "--verify", "HEAD"])
            .status
            .success()
    );
}

#[test]
fn update_commits_unrelated_staged_unstaged_and_untracked_content_but_not_ignored_files() {
    let repo = TestRepo::initialized();
    fs::write(repo.path().join(".gitignore"), "ignored.log\n").unwrap();
    fs::write(repo.path().join("note.md"), "before\n").unwrap();
    fs::write(repo.path().join("partial.md"), "before\n").unwrap();
    repo.git
        .commit_all(CommitIntent::Create(PathBuf::from("note.md")))
        .unwrap();

    fs::write(repo.path().join("note.md"), "after\n").unwrap();
    fs::write(repo.path().join("partial.md"), "staged version\n").unwrap();
    repo.git_ok(["add", "partial.md"]);
    fs::write(repo.path().join("partial.md"), "worktree version\n").unwrap();
    fs::write(repo.path().join("unstaged.md"), "unstaged\n").unwrap();
    fs::write(repo.path().join("untracked.md"), "untracked\n").unwrap();
    fs::write(repo.path().join("ignored.log"), "ignored\n").unwrap();

    let outcome = repo
        .git
        .commit_all(CommitIntent::Update(PathBuf::from("note.md")))
        .unwrap();

    assert!(matches!(outcome, CommitOutcome::Committed { .. }));
    assert_eq!(repo.subjects()[0], "carnet: update note.md");
    assert_eq!(repo.show("HEAD:note.md"), "after\n");
    assert_eq!(repo.show("HEAD:partial.md"), "worktree version\n");
    assert_eq!(repo.show("HEAD:unstaged.md"), "unstaged\n");
    assert_eq!(repo.show("HEAD:untracked.md"), "untracked\n");
    assert!(
        !repo
            .git_command(["cat-file", "-e", "HEAD:ignored.log"])
            .status
            .success()
    );
    assert_eq!(repo.status(), "");
    assert_eq!(
        fs::read_to_string(repo.path().join("ignored.log")).unwrap(),
        "ignored\n"
    );
}

#[test]
fn move_and_delete_use_exact_messages_and_commit_the_resulting_tree() {
    let repo = TestRepo::initialized();
    fs::write(repo.path().join("old.md"), "content\n").unwrap();
    repo.git
        .commit_all(CommitIntent::Create(PathBuf::from("old.md")))
        .unwrap();

    fs::rename(repo.path().join("old.md"), repo.path().join("new.md")).unwrap();
    repo.git
        .commit_all(CommitIntent::Move {
            from: PathBuf::from("old.md"),
            to: PathBuf::from("new.md"),
        })
        .unwrap();
    fs::remove_file(repo.path().join("new.md")).unwrap();
    repo.git
        .commit_all(CommitIntent::Delete(PathBuf::from("new.md")))
        .unwrap();

    assert_eq!(
        repo.subjects(),
        vec![
            "carnet: delete new.md",
            "carnet: move old.md to new.md",
            "carnet: create old.md",
        ]
    );
    assert!(
        !repo
            .git_command(["cat-file", "-e", "HEAD:new.md"])
            .status
            .success()
    );
}

#[test]
fn initialize_uses_the_same_configured_default_branch_as_ordinary_git_init() {
    let carnet_repo = TestRepo::initialized();
    let comparison = tempdir().unwrap();
    let output = Command::new("git")
        .arg("init")
        .arg(comparison.path())
        .output()
        .unwrap();
    assert!(output.status.success());

    let carnet_branch = carnet_repo
        .git_ok(["symbolic-ref", "--short", "HEAD"])
        .stdout;
    let comparison_branch = Command::new("git")
        .args(["symbolic-ref", "--short", "HEAD"])
        .current_dir(comparison.path())
        .output()
        .unwrap();

    assert!(comparison_branch.status.success());
    assert_eq!(carnet_branch, comparison_branch.stdout);
}

#[test]
fn open_accepts_a_work_tree_subdirectory_and_rejects_a_non_repository() {
    let repo = TestRepo::initialized();
    fs::create_dir(repo.path().join("notes")).unwrap();
    GitRepo::open(&repo.path().join("notes")).unwrap();

    let outside = tempdir().unwrap();
    assert!(GitRepo::open(outside.path()).is_err());
}

#[test]
fn saving_identical_content_does_not_create_an_empty_commit() {
    let repo = TestRepo::initialized();
    fs::write(repo.path().join("note.md"), "same\n").unwrap();
    repo.git
        .commit_all(CommitIntent::Create(PathBuf::from("note.md")))
        .unwrap();
    let workspace = open_workspace(repo.path());
    let note = workspace
        .load_note(&workspace.resolve_note(Path::new("note.md")).unwrap())
        .unwrap();

    let outcome = apply_and_commit(
        &workspace,
        &repo.git,
        FileOperation::Save {
            note,
            content: "same\n".into(),
            overwrite: false,
        },
        CommitIntent::Update(PathBuf::from("note.md")),
    )
    .unwrap();

    assert!(matches!(
        outcome,
        MutationCommitOutcome::Applied {
            commit: CommitOutcome::NoChanges,
            ..
        }
    ));
    assert_eq!(repo.subjects(), vec!["carnet: create note.md"]);
}

#[test]
fn missing_identity_returns_a_commit_error_and_keeps_changes_staged() {
    let repo = TestRepo::initialized();
    repo.git_ok(["config", "user.name", ""]);
    repo.git_ok(["config", "user.email", ""]);
    fs::write(repo.path().join("note.md"), "saved\n").unwrap();

    let error = repo
        .git
        .commit_all(CommitIntent::Create(PathBuf::from("note.md")))
        .unwrap_err();

    assert!(error.to_string().contains("commit"));
    assert_eq!(repo.status(), "A  note.md\n");
    assert_eq!(
        fs::read_to_string(repo.path().join("note.md")).unwrap(),
        "saved\n"
    );
}

#[cfg(unix)]
#[test]
fn a_rejected_commit_returns_an_error_and_can_be_retried() {
    use std::os::unix::fs::PermissionsExt;

    let repo = TestRepo::initialized();
    fs::write(repo.path().join("note.md"), "saved\n").unwrap();
    let hook = repo.path().join(".git/hooks/pre-commit");
    fs::write(&hook, "#!/bin/sh\necho rejected >&2\nexit 1\n").unwrap();
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();

    let error = repo
        .git
        .commit_all(CommitIntent::Create(PathBuf::from("note.md")))
        .unwrap_err();

    assert!(error.to_string().contains("rejected"));
    assert_eq!(repo.status(), "A  note.md\n");
    fs::remove_file(hook).unwrap();
    let retry = repo
        .git
        .commit_all(CommitIntent::Create(PathBuf::from("note.md")))
        .unwrap();
    assert!(matches!(retry, CommitOutcome::Committed { .. }));
    assert_eq!(repo.subjects(), vec!["carnet: create note.md"]);
}

#[test]
fn a_successful_save_is_preserved_when_commit_fails_and_retry_does_not_rewrite_it() {
    #[cfg(unix)]
    use std::os::unix::fs::MetadataExt;

    let repo = TestRepo::initialized();
    let note_path = repo.path().join("note.md");
    fs::write(&note_path, "before\n").unwrap();
    repo.git
        .commit_all(CommitIntent::Create(PathBuf::from("note.md")))
        .unwrap();
    let workspace = open_workspace(repo.path());
    let note = workspace
        .load_note(&workspace.resolve_note(Path::new("note.md")).unwrap())
        .unwrap();
    repo.git_ok(["config", "user.name", ""]);
    repo.git_ok(["config", "user.email", ""]);
    let intent = CommitIntent::Update(PathBuf::from("note.md"));

    let outcome = apply_and_commit(
        &workspace,
        &repo.git,
        FileOperation::Save {
            note,
            content: "after\n".into(),
            overwrite: false,
        },
        intent.clone(),
    )
    .unwrap();

    match outcome {
        MutationCommitOutcome::SavedCommitFailed { file, error } => {
            assert!(matches!(file, carnet::workspace::FileOutcome::Saved(_)));
            assert!(error.to_string().contains("commit"));
        }
        other => panic!("expected saved commit failure, got {other:?}"),
    }
    assert_eq!(fs::read(&note_path).unwrap(), b"after\n");
    let saved_metadata = fs::metadata(&note_path).unwrap();

    repo.git_ok(["config", "user.name", "Carnet Test"]);
    repo.git_ok(["config", "user.email", "carnet@example.test"]);
    let retry = repo.git.commit_all(intent).unwrap();

    assert!(matches!(retry, CommitOutcome::Committed { .. }));
    assert_eq!(fs::read(&note_path).unwrap(), b"after\n");
    #[cfg(unix)]
    assert_eq!(
        fs::metadata(&note_path).unwrap().ino(),
        saved_metadata.ino()
    );
    assert_eq!(repo.subjects()[0], "carnet: update note.md");
}

#[test]
fn a_filesystem_conflict_is_an_error_and_does_not_attempt_a_commit() {
    let repo = TestRepo::initialized();
    let note_path = repo.path().join("note.md");
    fs::write(&note_path, "loaded\n").unwrap();
    repo.git
        .commit_all(CommitIntent::Create(PathBuf::from("note.md")))
        .unwrap();
    let workspace = open_workspace(repo.path());
    let note = workspace
        .load_note(&workspace.resolve_note(Path::new("note.md")).unwrap())
        .unwrap();
    fs::write(&note_path, "external\n").unwrap();

    let result = apply_and_commit(
        &workspace,
        &repo.git,
        FileOperation::Save {
            note,
            content: "editor\n".into(),
            overwrite: false,
        },
        CommitIntent::Update(PathBuf::from("note.md")),
    );

    assert!(result.is_err());
    assert_eq!(fs::read(&note_path).unwrap(), b"external\n");
    assert_eq!(repo.subjects(), vec!["carnet: create note.md"]);
    assert_eq!(repo.status(), " M note.md\n");
}

#[test]
fn rejects_an_operation_from_another_workspace_before_mutating_or_staging_either_repo() {
    let repo_a = TestRepo::initialized_with_commit("a.md");
    let repo_b = TestRepo::initialized_with_commit("b.md");
    let workspace_a = open_workspace(repo_a.path());
    let workspace_b = open_workspace(repo_b.path());

    let result = apply_and_commit(
        &workspace_a,
        &repo_a.git,
        FileOperation::CreateFile {
            workspace: workspace_b,
            path: PathBuf::from("wrong-repo.md"),
        },
        CommitIntent::Create(PathBuf::from("wrong-repo.md")),
    );

    assert!(matches!(
        result,
        Err(MutationCommitError::WorkspaceMismatch)
    ));
    assert!(!repo_a.path().join("wrong-repo.md").exists());
    assert!(!repo_b.path().join("wrong-repo.md").exists());
    assert_eq!(repo_a.status(), "");
    assert_eq!(repo_b.status(), "");
    assert_eq!(repo_a.subjects(), vec!["carnet: create a.md"]);
    assert_eq!(repo_b.subjects(), vec!["carnet: create b.md"]);
}

#[test]
fn rejects_a_git_repo_from_another_workspace_before_mutating_or_staging_either_repo() {
    let repo_a = TestRepo::initialized_with_commit("a.md");
    let repo_b = TestRepo::initialized_with_commit("b.md");
    let workspace_a = open_workspace(repo_a.path());

    let result = apply_and_commit(
        &workspace_a,
        &repo_b.git,
        FileOperation::CreateFile {
            workspace: workspace_a.clone(),
            path: PathBuf::from("wrong-repo.md"),
        },
        CommitIntent::Create(PathBuf::from("wrong-repo.md")),
    );

    assert!(matches!(
        result,
        Err(MutationCommitError::RepositoryMismatch)
    ));
    assert!(!repo_a.path().join("wrong-repo.md").exists());
    assert!(!repo_b.path().join("wrong-repo.md").exists());
    assert_eq!(repo_a.status(), "");
    assert_eq!(repo_b.status(), "");
    assert_eq!(repo_a.subjects(), vec!["carnet: create a.md"]);
    assert_eq!(repo_b.subjects(), vec!["carnet: create b.md"]);
}

struct TestRepo {
    _temp: TempDir,
    git: GitRepo,
}

fn open_workspace(root: &Path) -> Workspace {
    Workspace::open(RepoEntry {
        id: Uuid::new_v4(),
        name: "notes".into(),
        path: fs::canonicalize(root).unwrap(),
    })
    .unwrap()
}

impl TestRepo {
    fn initialized() -> Self {
        let temp = tempdir().unwrap();
        let git = GitRepo::initialize(temp.path()).unwrap();
        let repo = Self { _temp: temp, git };
        repo.git_ok(["config", "user.name", "Carnet Test"]);
        repo.git_ok(["config", "user.email", "carnet@example.test"]);
        repo
    }

    fn initialized_with_commit(path: &str) -> Self {
        let repo = Self::initialized();
        fs::write(repo.path().join(path), "baseline\n").unwrap();
        repo.git
            .commit_all(CommitIntent::Create(PathBuf::from(path)))
            .unwrap();
        repo
    }

    fn path(&self) -> &Path {
        self._temp.path()
    }

    fn git_command<const N: usize>(&self, args: [&str; N]) -> Output {
        Command::new("git")
            .args(args)
            .current_dir(self.path())
            .output()
            .unwrap()
    }

    fn git_ok<const N: usize>(&self, args: [&str; N]) -> Output {
        let output = self.git_command(args);
        assert!(
            output.status.success(),
            "git failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }

    fn subjects(&self) -> Vec<String> {
        String::from_utf8(self.git_ok(["log", "--format=%s"]).stdout)
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    fn show(&self, object: &str) -> String {
        String::from_utf8(self.git_ok(["show", object]).stdout).unwrap()
    }

    fn status(&self) -> String {
        String::from_utf8(self.git_ok(["status", "--porcelain=v1"]).stdout).unwrap()
    }
}
