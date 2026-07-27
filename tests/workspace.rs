use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use carnet::{
    catalog::RepoEntry,
    workspace::{
        FileError, FileOperation, FileOutcome, NewlineStyle, PathError, TreeEntry, TreeEntryKind,
        Workspace,
    },
};
use tempfile::tempdir;
use uuid::Uuid;

use proptest::prelude::*;

#[test]
fn rejects_absolute_traversal_and_git_note_paths() {
    let sandbox = tempdir().unwrap();
    let workspace = Workspace::open(RepoEntry {
        id: Uuid::new_v4(),
        name: "notes".into(),
        path: fs::canonicalize(sandbox.path()).unwrap(),
    })
    .unwrap();

    let cases = [
        (PathBuf::from("/tmp/outside.md"), "absolute"),
        (PathBuf::from("../outside.md"), "traversal"),
        (PathBuf::from("notes/../../outside.md"), "traversal"),
        (PathBuf::from(".git/config"), "git"),
        (PathBuf::from("notes/.git/config"), "git"),
    ];

    for (path, kind) in cases {
        let error = workspace.resolve_note(Path::new(&path)).unwrap_err();
        assert_eq!(error.kind(), kind, "path: {}", path.display());
        assert!(matches!(
            error,
            PathError::Absolute { .. }
                | PathError::Traversal { .. }
                | PathError::GitMetadata { .. }
        ));
    }
}

#[test]
fn rejects_git_metadata_components_case_insensitively() {
    let sandbox = tempdir().unwrap();
    let root = fs::canonicalize(sandbox.path()).unwrap();
    let workspace = open_workspace(root);

    for path in [".GIT/config", "notes/.Git/index", "nested/.gIt/HEAD"] {
        assert!(matches!(
            workspace.resolve_note(Path::new(path)),
            Err(PathError::GitMetadata { .. })
        ));
    }
}

#[test]
fn save_reports_external_modification_and_deletion_conflicts_unless_overwritten() {
    let sandbox = tempdir().unwrap();
    let root = fs::canonicalize(sandbox.path()).unwrap();
    fs::write(root.join("modified.md"), "loaded").unwrap();
    fs::write(root.join("deleted.md"), "loaded").unwrap();
    let workspace = open_workspace(root.clone());
    let modified = workspace
        .load_note(&workspace.resolve_note(Path::new("modified.md")).unwrap())
        .unwrap();
    let deleted = workspace
        .load_note(&workspace.resolve_note(Path::new("deleted.md")).unwrap())
        .unwrap();
    fs::write(root.join("modified.md"), "external").unwrap();
    fs::remove_file(root.join("deleted.md")).unwrap();

    let modified_error = Workspace::apply(FileOperation::Save {
        note: modified.clone(),
        content: "editor".into(),
        overwrite: false,
    })
    .unwrap_err();
    let deleted_error = Workspace::apply(FileOperation::Save {
        note: deleted,
        content: "editor".into(),
        overwrite: false,
    })
    .unwrap_err();

    assert!(matches!(
        modified_error,
        FileError::ExternalModification { .. }
    ));
    assert!(matches!(deleted_error, FileError::ExternalDeletion { .. }));
    assert_eq!(
        fs::read_to_string(root.join("modified.md")).unwrap(),
        "external"
    );
    assert!(!root.join("deleted.md").exists());

    Workspace::apply(FileOperation::Save {
        note: modified,
        content: "editor".into(),
        overwrite: true,
    })
    .unwrap();
    assert_eq!(
        fs::read_to_string(root.join("modified.md")).unwrap(),
        "editor"
    );
}

#[test]
fn loads_utf8_notes_with_round_trip_metadata_and_keeps_missing_notes_unsaved() {
    struct LoadCase {
        name: &'static str,
        bytes: &'static [u8],
        text: &'static str,
        bom: bool,
        newline: NewlineStyle,
        final_newline: bool,
    }

    let sandbox = tempdir().unwrap();
    let root = fs::canonicalize(sandbox.path()).unwrap();
    let cases = [
        LoadCase {
            name: "empty.md",
            bytes: b"",
            text: "",
            bom: false,
            newline: NewlineStyle::Lf,
            final_newline: false,
        },
        LoadCase {
            name: "bom.md",
            bytes: b"\xef\xbb\xbfhello\n",
            text: "hello\n",
            bom: true,
            newline: NewlineStyle::Lf,
            final_newline: true,
        },
        LoadCase {
            name: "lf.md",
            bytes: b"one\ntwo",
            text: "one\ntwo",
            bom: false,
            newline: NewlineStyle::Lf,
            final_newline: false,
        },
        LoadCase {
            name: "crlf.md",
            bytes: b"one\r\ntwo\r\n",
            text: "one\ntwo\n",
            bom: false,
            newline: NewlineStyle::CrLf,
            final_newline: true,
        },
        LoadCase {
            name: "unicode.md",
            bytes: "olá 🌍\n".as_bytes(),
            text: "olá 🌍\n",
            bom: false,
            newline: NewlineStyle::Lf,
            final_newline: true,
        },
    ];
    for case in &cases {
        fs::write(root.join(case.name), case.bytes).unwrap();
    }
    fs::write(root.join("long.md"), "x".repeat(128 * 1024)).unwrap();
    let workspace = open_workspace(root);

    for case in &cases {
        let path = workspace.resolve_note(Path::new(case.name)).unwrap();
        let loaded = workspace.load_note(&path).unwrap();
        assert_eq!(loaded.text(), case.text, "case: {}", case.name);
        assert_eq!(loaded.has_bom(), case.bom, "case: {}", case.name);
        assert_eq!(loaded.newline_style(), case.newline, "case: {}", case.name);
        assert_eq!(
            loaded.had_final_newline(),
            case.final_newline,
            "case: {}",
            case.name
        );
        assert!(loaded.content_hash().is_some(), "case: {}", case.name);
        assert!(loaded.is_saved(), "case: {}", case.name);
    }

    let long = workspace
        .load_note(&workspace.resolve_note(Path::new("long.md")).unwrap())
        .unwrap();
    assert_eq!(long.text().len(), 128 * 1024);

    let missing = workspace
        .load_note(&workspace.resolve_note(Path::new("new/note.md")).unwrap())
        .unwrap();
    assert_eq!(missing.text(), "");
    assert!(!missing.is_saved());
    assert_eq!(missing.content_hash(), None);
}

#[test]
fn load_rejects_binary_and_invalid_utf8_files_with_typed_errors() {
    let sandbox = tempdir().unwrap();
    let root = fs::canonicalize(sandbox.path()).unwrap();
    fs::write(root.join("binary.dat"), b"text\0more").unwrap();
    fs::write(root.join("invalid.dat"), b"\xf0\x28\x8c\x28").unwrap();
    let workspace = open_workspace(root);

    let binary = workspace
        .load_note(&workspace.resolve_note(Path::new("binary.dat")).unwrap())
        .unwrap_err();
    let invalid = workspace
        .load_note(&workspace.resolve_note(Path::new("invalid.dat")).unwrap())
        .unwrap_err();

    assert!(matches!(binary, FileError::Binary { .. }));
    assert!(matches!(invalid, FileError::InvalidUtf8 { .. }));
}

#[cfg(unix)]
#[test]
fn failed_atomic_save_leaves_original_bytes_untouched() {
    use std::os::unix::fs::PermissionsExt;

    let sandbox = tempdir().unwrap();
    let root = fs::canonicalize(sandbox.path()).unwrap();
    let folder = root.join("locked");
    fs::create_dir(&folder).unwrap();
    let target = folder.join("note.md");
    fs::write(&target, "original").unwrap();
    let workspace = open_workspace(root);
    let note = workspace
        .load_note(&workspace.resolve_note(Path::new("locked/note.md")).unwrap())
        .unwrap();
    fs::set_permissions(&folder, fs::Permissions::from_mode(0o555)).unwrap();

    let result = Workspace::apply(FileOperation::Save {
        note,
        content: "replacement".into(),
        overwrite: false,
    });
    fs::set_permissions(&folder, fs::Permissions::from_mode(0o755)).unwrap();

    assert!(matches!(result, Err(FileError::Io { .. })));
    assert_eq!(fs::read_to_string(target).unwrap(), "original");
}

fn open_workspace(root: PathBuf) -> Workspace {
    Workspace::open(RepoEntry {
        id: Uuid::new_v4(),
        name: "notes".into(),
        path: root,
    })
    .unwrap()
}

proptest! {
    #[test]
    fn hostile_components_never_resolve_or_create_outside_the_root(
        before in prop::collection::vec("[a-zA-Z0-9_-]{1,12}", 0..6),
        after in prop::collection::vec("[a-zA-Z0-9_-]{1,12}", 0..6),
        hostile in prop_oneof![Just(".."), Just(".git")],
    ) {
        let sandbox = tempdir().unwrap();
        let root = fs::canonicalize(sandbox.path()).unwrap();
        let outside = root.parent().unwrap().join(format!("outside-{}", Uuid::new_v4()));
        let workspace = open_workspace(root.clone());
        let path = before
            .iter()
            .map(String::as_str)
            .chain(std::iter::once(hostile))
            .chain(after.iter().map(String::as_str))
            .chain(std::iter::once("note.md"))
            .collect::<PathBuf>();

        prop_assert!(workspace.resolve_note(&path).is_err());
        let create = Workspace::apply(FileOperation::CreateFile {
            workspace,
            path,
        });
        prop_assert!(create.is_err());
        prop_assert!(!outside.exists());
    }
}

#[test]
fn tree_hides_git_and_ignored_paths_but_disables_non_text_and_symlinks() {
    let sandbox = tempdir().unwrap();
    let root = fs::canonicalize(sandbox.path()).unwrap();
    assert!(
        Command::new("git")
            .arg("init")
            .arg(&root)
            .output()
            .unwrap()
            .status
            .success()
    );
    fs::write(root.join(".gitignore"), "ignored*\nnested/hidden.md\n").unwrap();
    fs::write(root.join("note.md"), "hello").unwrap();
    fs::write(root.join("binary.dat"), b"a\0b").unwrap();
    fs::write(root.join("invalid.dat"), b"\xff\xfe").unwrap();
    fs::write(root.join("ignored.log"), "hidden").unwrap();
    fs::create_dir(root.join("empty-folder")).unwrap();
    fs::create_dir(root.join("nested")).unwrap();
    fs::write(root.join("nested/hidden.md"), "hidden").unwrap();
    fs::write(root.join("nested/shown.md"), "shown").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(root.join("note.md"), root.join("link.md")).unwrap();

    let entries = open_workspace(root).tree().unwrap();
    let flattened = flatten_tree(&entries);

    assert_eq!(
        flattened.get(Path::new("note.md")),
        Some(&(TreeEntryKind::File, true))
    );
    assert_eq!(
        flattened.get(Path::new("binary.dat")),
        Some(&(TreeEntryKind::File, false))
    );
    assert_eq!(
        flattened.get(Path::new("invalid.dat")),
        Some(&(TreeEntryKind::File, false))
    );
    assert_eq!(
        flattened.get(Path::new("empty-folder")),
        Some(&(TreeEntryKind::Directory, true))
    );
    assert_eq!(
        flattened.get(Path::new("nested/shown.md")),
        Some(&(TreeEntryKind::File, true))
    );
    #[cfg(unix)]
    assert_eq!(
        flattened.get(Path::new("link.md")),
        Some(&(TreeEntryKind::Symlink, false))
    );
    assert!(!flattened.contains_key(Path::new(".git")));
    assert!(!flattened.contains_key(Path::new("ignored.log")));
    assert!(!flattened.contains_key(Path::new("nested/hidden.md")));
}

#[test]
fn tree_keeps_tracked_files_inside_an_ignored_directory() {
    let sandbox = tempdir().unwrap();
    let root = fs::canonicalize(sandbox.path()).unwrap();
    assert!(
        Command::new("git")
            .arg("init")
            .arg(&root)
            .output()
            .unwrap()
            .status
            .success()
    );
    fs::create_dir(root.join("ignored")).unwrap();
    fs::write(root.join("ignored/kept.md"), "tracked").unwrap();
    fs::write(root.join("ignored/hidden.md"), "untracked").unwrap();
    assert!(
        Command::new("git")
            .args(["-C", root.to_str().unwrap(), "add", "ignored/kept.md"])
            .output()
            .unwrap()
            .status
            .success()
    );
    fs::write(root.join(".gitignore"), "ignored/\n").unwrap();

    let flattened = flatten_tree(&open_workspace(root).tree().unwrap());

    assert!(flattened.contains_key(Path::new("ignored")));
    assert!(flattened.contains_key(Path::new("ignored/kept.md")));
    assert!(!flattened.contains_key(Path::new("ignored/hidden.md")));
}

#[test]
fn tree_returns_an_error_when_git_ignore_check_is_fatal() {
    let sandbox = tempdir().unwrap();
    let root = fs::canonicalize(sandbox.path()).unwrap();
    fs::write(root.join(".git"), "not a valid gitfile\n").unwrap();
    fs::write(root.join("note.md"), "visible only if Git succeeds").unwrap();

    let error = open_workspace(root).tree().unwrap_err();

    assert!(matches!(error, FileError::GitIgnore { .. }));
}

fn flatten_tree(
    entries: &[TreeEntry],
) -> std::collections::BTreeMap<PathBuf, (TreeEntryKind, bool)> {
    fn visit(
        entries: &[TreeEntry],
        flattened: &mut std::collections::BTreeMap<PathBuf, (TreeEntryKind, bool)>,
    ) {
        for entry in entries {
            flattened.insert(
                entry.path().to_path_buf(),
                (entry.kind(), entry.is_enabled()),
            );
            visit(entry.children(), flattened);
        }
    }

    let mut flattened = std::collections::BTreeMap::new();
    visit(entries, &mut flattened);
    flattened
}

#[test]
fn applies_create_rename_move_and_confirmed_delete_operations() {
    let sandbox = tempdir().unwrap();
    let root = fs::canonicalize(sandbox.path()).unwrap();
    let workspace = open_workspace(root.clone());

    assert!(matches!(
        Workspace::apply(FileOperation::CreateFolder {
            workspace: workspace.clone(),
            path: PathBuf::from("ideas"),
        })
        .unwrap(),
        FileOutcome::CreatedFolder(path) if path == Path::new("ideas")
    ));
    assert!(root.join("ideas").is_dir());
    assert!(!root.join("ideas/.gitkeep").exists());

    assert!(matches!(
        Workspace::apply(FileOperation::CreateFile {
            workspace: workspace.clone(),
            path: PathBuf::from("ideas/draft.md"),
        })
        .unwrap(),
        FileOutcome::CreatedFile(path) if path.relative() == Path::new("ideas/draft.md")
    ));
    assert_eq!(fs::read(root.join("ideas/draft.md")).unwrap(), b"");

    Workspace::apply(FileOperation::Rename {
        workspace: workspace.clone(),
        from: PathBuf::from("ideas/draft.md"),
        to: PathBuf::from("ideas/outline.md"),
    })
    .unwrap();
    fs::create_dir(root.join("archive")).unwrap();
    Workspace::apply(FileOperation::Move {
        workspace: workspace.clone(),
        from: PathBuf::from("ideas/outline.md"),
        to: PathBuf::from("archive/outline.md"),
    })
    .unwrap();
    assert!(root.join("archive/outline.md").is_file());

    let error = Workspace::apply(FileOperation::Delete {
        workspace: workspace.clone(),
        path: PathBuf::from("archive"),
        confirmed: false,
    })
    .unwrap_err();
    assert!(matches!(error, FileError::ConfirmationRequired { .. }));
    assert!(root.join("archive/outline.md").exists());

    Workspace::apply(FileOperation::Delete {
        workspace,
        path: PathBuf::from("archive"),
        confirmed: true,
    })
    .unwrap();
    assert!(!root.join("archive").exists());
}

#[test]
fn save_atomically_preserves_format_and_permissions_and_creates_missing_parents() {
    #[cfg(unix)]
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let sandbox = tempdir().unwrap();
    let root = fs::canonicalize(sandbox.path()).unwrap();
    let existing_path = root.join("existing.md");
    fs::write(&existing_path, b"\xef\xbb\xbforiginal\r\n").unwrap();
    #[cfg(unix)]
    fs::set_permissions(&existing_path, fs::Permissions::from_mode(0o640)).unwrap();
    let workspace = open_workspace(root.clone());
    let existing = workspace
        .load_note(&workspace.resolve_note(Path::new("existing.md")).unwrap())
        .unwrap();
    #[cfg(unix)]
    let original_inode = fs::metadata(&existing_path).unwrap().ino();

    let outcome = Workspace::apply(FileOperation::Save {
        note: existing,
        content: "changed\nsecond".into(),
        overwrite: false,
    })
    .unwrap();

    assert_eq!(
        fs::read(&existing_path).unwrap(),
        b"\xef\xbb\xbfchanged\r\nsecond\r\n"
    );
    #[cfg(unix)]
    {
        let metadata = fs::metadata(&existing_path).unwrap();
        assert_eq!(metadata.permissions().mode() & 0o777, 0o640);
        assert_ne!(metadata.ino(), original_inode);
    }
    let FileOutcome::Saved(saved) = outcome else {
        panic!("expected save outcome");
    };
    assert!(saved.content_hash().is_some());

    let missing = workspace
        .load_note(
            &workspace
                .resolve_note(Path::new("new/deep/note.md"))
                .unwrap(),
        )
        .unwrap();
    let outcome = Workspace::apply(FileOperation::Save {
        note: missing,
        content: "first save\n".into(),
        overwrite: false,
    })
    .unwrap();
    assert_eq!(
        fs::read(root.join("new/deep/note.md")).unwrap(),
        b"first save\n"
    );
    assert!(matches!(outcome, FileOutcome::Saved(note) if note.is_saved()));
}

#[test]
fn rejects_directory_targets_and_symlink_components() {
    let sandbox = tempdir().unwrap();
    let root = fs::canonicalize(sandbox.path()).unwrap();
    fs::create_dir(root.join("folder")).unwrap();
    let outside = tempdir().unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(outside.path(), root.join("escape")).unwrap();
    let workspace = Workspace::open(RepoEntry {
        id: Uuid::new_v4(),
        name: "notes".into(),
        path: root,
    })
    .unwrap();

    assert!(matches!(
        workspace.resolve_note(Path::new("")),
        Err(PathError::DirectoryTarget { .. })
    ));
    assert!(matches!(
        workspace.resolve_note(Path::new("folder")),
        Err(PathError::DirectoryTarget { .. })
    ));
    #[cfg(unix)]
    assert!(matches!(
        workspace.resolve_note(Path::new("escape/new.md")),
        Err(PathError::Symlink { .. })
    ));
}

#[cfg(unix)]
#[test]
fn mutations_reject_a_repository_root_replaced_by_a_symlink() {
    let sandbox = tempdir().unwrap();
    let repo = sandbox.path().join("repo");
    fs::create_dir(&repo).unwrap();
    let workspace = open_workspace(fs::canonicalize(&repo).unwrap());
    let outside = tempdir().unwrap();
    fs::remove_dir(&repo).unwrap();
    std::os::unix::fs::symlink(outside.path(), &repo).unwrap();

    assert!(workspace.tree().is_err());
    Workspace::apply(FileOperation::CreateFile {
        workspace,
        path: PathBuf::from("escaped.md"),
    })
    .unwrap_err();

    assert!(!outside.path().join("escaped.md").exists());
}

#[cfg(unix)]
#[test]
fn mutations_remain_bound_to_the_opened_root_directory_identity() {
    let sandbox = tempdir().unwrap();
    let repo = sandbox.path().join("repo");
    let moved_repo = sandbox.path().join("moved-repo");
    fs::create_dir(&repo).unwrap();
    let workspace = open_workspace(fs::canonicalize(&repo).unwrap());
    fs::write(repo.join("save.md"), "loaded").unwrap();
    fs::write(repo.join("rename.md"), "rename").unwrap();
    fs::write(repo.join("move.md"), "move").unwrap();
    fs::write(repo.join("delete.md"), "delete").unwrap();
    fs::create_dir(repo.join("archive")).unwrap();
    let loaded = workspace
        .load_note(&workspace.resolve_note(Path::new("save.md")).unwrap())
        .unwrap();
    let outside = tempdir().unwrap();
    fs::rename(&repo, &moved_repo).unwrap();
    std::os::unix::fs::symlink(outside.path(), &repo).unwrap();

    Workspace::apply(FileOperation::CreateFile {
        workspace: workspace.clone(),
        path: PathBuf::from("confined.md"),
    })
    .unwrap();
    Workspace::apply(FileOperation::CreateFolder {
        workspace: workspace.clone(),
        path: PathBuf::from("created-folder"),
    })
    .unwrap();
    Workspace::apply(FileOperation::Save {
        note: loaded,
        content: "saved".into(),
        overwrite: false,
    })
    .unwrap();
    Workspace::apply(FileOperation::Rename {
        workspace: workspace.clone(),
        from: PathBuf::from("rename.md"),
        to: PathBuf::from("renamed.md"),
    })
    .unwrap();
    Workspace::apply(FileOperation::Move {
        workspace: workspace.clone(),
        from: PathBuf::from("move.md"),
        to: PathBuf::from("archive/moved.md"),
    })
    .unwrap();
    Workspace::apply(FileOperation::Delete {
        workspace,
        path: PathBuf::from("delete.md"),
        confirmed: true,
    })
    .unwrap();

    assert!(moved_repo.join("confined.md").is_file());
    assert!(moved_repo.join("created-folder").is_dir());
    assert_eq!(
        fs::read_to_string(moved_repo.join("save.md")).unwrap(),
        "saved"
    );
    assert!(moved_repo.join("renamed.md").is_file());
    assert!(moved_repo.join("archive/moved.md").is_file());
    assert!(!moved_repo.join("delete.md").exists());
    assert!(!outside.path().join("confined.md").exists());
    assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 0);
}
