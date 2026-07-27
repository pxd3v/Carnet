use std::{fs, path::PathBuf};

use carnet::{
    app::{
        App, AppAction, AppEffect, AppEvent, CatalogSnapshot, DefaultChoiceState, HomeAction,
        PendingFileMutation, PendingIntent, Screen,
    },
    catalog::RepoEntry,
    editor::EditorCommand,
    git::GitRepo,
    workspace::Workspace,
};
use tempfile::{TempDir, tempdir};
use uuid::Uuid;

#[test]
fn home_selection_changes_pure_state_and_opening_emits_the_selected_repository() {
    let first = repository(1, "first");
    let second = repository(2, "second");
    let mut app = App::home(vec![first.clone(), second.clone()], Some(first.id), None);

    assert!(matches!(app.screen, Screen::Home));
    assert_eq!(app.home.selected, Some(0));

    assert!(
        app.update(AppEvent::Action(AppAction::Home(HomeAction::Down)))
            .is_empty()
    );
    app.update(AppEvent::Action(AppAction::Home(HomeAction::Up)));
    assert_eq!(app.home.selected, Some(0));
    app.update(AppEvent::Action(AppAction::Home(HomeAction::Down)));
    let effects = app.update(AppEvent::Action(AppAction::Home(HomeAction::OpenSelected)));

    assert_eq!(app.home.selected, Some(1));
    assert_eq!(effects.len(), 1);
    match &effects[0] {
        AppEffect::OpenWorkspace {
            repository, note, ..
        } => {
            assert_eq!(repository, &second);
            assert_eq!(note, &None);
        }
        other => panic!("unexpected effect: {other:?}"),
    }
}

#[test]
fn global_save_emits_one_apply_and_commit_effect_and_suppresses_competing_saves() {
    use carnet::{
        app::{GlobalAction, PendingMutationKind},
        git::CommitIntent,
        workspace::FileOperation,
    };

    let (_sandbox, mut app) = app_with_note(22, "note.md", "base");
    app.update(AppEvent::Action(AppAction::Editor(EditorCommand::Insert(
        "x".into(),
    ))));

    let effects = app.update(AppEvent::Action(AppAction::Global(GlobalAction::Save)));

    assert!(matches!(
        app.pending_mutation.as_ref().map(|pending| &pending.kind),
        Some(PendingMutationKind::Save { overwrite: false })
    ));
    assert_eq!(effects.len(), 1);
    match mutation_parts(&effects[0]) {
        (
            repository_id,
            FileOperation::Save {
                note,
                content,
                overwrite,
            },
            intent,
        ) => {
            assert_eq!(repository_id, Uuid::from_u128(22));
            assert_eq!(note.path().relative(), PathBuf::from("note.md").as_path());
            assert_eq!(content, "xbase");
            assert!(!overwrite);
            assert_eq!(intent, &CommitIntent::Update(PathBuf::from("note.md")));
        }
        other => panic!("unexpected effect: {other:?}"),
    }

    assert!(
        app.update(AppEvent::Action(AppAction::Global(GlobalAction::Save)))
            .is_empty()
    );
}

#[test]
fn pending_mutations_reject_clean_navigation_quit_and_dirty_discard() {
    use carnet::app::{
        DirtyChoice, FileActionKind, Focus, GlobalAction, NavigationAction, PendingMutationKind,
        TreeAction,
    };

    let (_sandbox, mut create_app) = app_with_note(70, "note.md", "base");
    create_app.update(AppEvent::Action(AppAction::Focus(Focus::Tree)));
    create_app.update(AppEvent::Action(AppAction::Tree(TreeAction::NewFile)));
    create_app.update(AppEvent::Action(AppAction::SubmitFileAction(
        PathBuf::from("new.md"),
    )));
    assert!(matches!(
        create_app
            .pending_mutation
            .as_ref()
            .map(|pending| pending.kind),
        Some(PendingMutationKind::File(FileActionKind::NewFile))
    ));

    assert!(
        create_app
            .update(AppEvent::Action(AppAction::Global(GlobalAction::Quit)))
            .is_empty()
    );
    assert!(!create_app.quit.requested);
    create_app.update(AppEvent::Action(AppAction::Navigate(
        NavigationAction::Home,
    )));
    assert!(matches!(create_app.screen, Screen::Workspace(_)));

    let (_sandbox, mut save_app) = app_with_note(71, "note.md", "base");
    save_app.update(AppEvent::Action(AppAction::Editor(EditorCommand::Insert(
        "dirty ".into(),
    ))));
    save_app.update(AppEvent::Action(AppAction::Navigate(
        NavigationAction::Quit,
    )));
    save_app.update(AppEvent::DirtyChoice(DirtyChoice::Save));

    assert!(
        save_app
            .update(AppEvent::DirtyChoice(DirtyChoice::Discard))
            .is_empty()
    );
    assert!(!save_app.quit.requested);
    assert_eq!(
        save_app.pending_intent,
        Some(PendingIntent::Navigation(NavigationAction::Quit))
    );
}

#[test]
fn dirty_create_file_waits_for_save_success_before_starting_the_create() {
    use carnet::{
        app::{Dialog, DirtyChoice, Focus, TreeAction},
        git::{CommitIntent, CommitOutcome},
        workspace::FileOperation,
    };

    let (_sandbox, mut app) = app_with_note(84, "note.md", "base");
    app.update(AppEvent::Action(AppAction::Editor(EditorCommand::Insert(
        "dirty ".into(),
    ))));
    app.update(AppEvent::Action(AppAction::Focus(Focus::Tree)));
    app.update(AppEvent::Action(AppAction::Tree(TreeAction::NewFile)));

    let premature_create = app.update(AppEvent::Action(AppAction::SubmitFileAction(
        PathBuf::from("created.md"),
    )));
    assert!(premature_create.is_empty());
    assert_eq!(app.dialog, Some(Dialog::DirtyNavigation));

    let save = app.update(AppEvent::DirtyChoice(DirtyChoice::Save));
    assert!(matches!(
        mutation_parts(&save[0]).1,
        FileOperation::Save { content, .. } if content == "dirty base"
    ));
    let (mutation_id, repository_id, repository_root, file) =
        apply_save_effect(save.into_iter().next().unwrap());

    let create = app.update(AppEvent::MutationApplied {
        mutation_id,
        repository_id,
        repository_root,
        file,
        commit: CommitOutcome::NoChanges,
        tree: Ok(Vec::new()),
    });
    assert!(matches!(
        mutation_parts(&create[0]),
        (
            _,
            FileOperation::CreateFile { path, .. },
            CommitIntent::Create(intent_path),
        ) if path == PathBuf::from("created.md").as_path()
            && intent_path == PathBuf::from("created.md").as_path()
    ));
    assert!(!workspace_editor(&app).is_dirty());
}

#[test]
fn dirty_create_file_reprompts_when_edits_arrive_during_its_prerequisite_save() {
    use carnet::{
        app::{Dialog, DirtyChoice, FileMutationAction, Focus, TreeAction},
        git::CommitOutcome,
    };

    let (_sandbox, mut app) = app_with_note(89, "note.md", "base");
    app.update(AppEvent::Action(AppAction::Editor(EditorCommand::Insert(
        "saved ".into(),
    ))));
    app.update(AppEvent::Action(AppAction::Focus(Focus::Tree)));
    app.update(AppEvent::Action(AppAction::Tree(TreeAction::NewFile)));
    app.update(AppEvent::Action(AppAction::SubmitFileAction(
        PathBuf::from("created.md"),
    )));
    let save = app.update(AppEvent::DirtyChoice(DirtyChoice::Save));
    app.update(AppEvent::Action(AppAction::Editor(EditorCommand::Insert(
        "newer ".into(),
    ))));
    let (mutation_id, repository_id, repository_root, file) =
        apply_save_effect(save.into_iter().next().unwrap());

    assert!(
        app.update(AppEvent::MutationApplied {
            mutation_id,
            repository_id,
            repository_root,
            file,
            commit: CommitOutcome::NoChanges,
            tree: Ok(Vec::new()),
        })
        .is_empty()
    );
    assert_eq!(app.dialog, Some(Dialog::DirtyNavigation));
    assert!(matches!(
        app.pending_intent.as_ref(),
        Some(PendingIntent::Mutation(PendingFileMutation {
            action: FileMutationAction::CreateFile { path },
            ..
        })) if path == PathBuf::from("created.md").as_path()
    ));
    assert_eq!(workspace_editor(&app).text(), "saved newer base");
    assert!(workspace_editor(&app).is_dirty());
    assert!(!matches!(
        app.pending_mutation.as_ref().map(|pending| pending.kind),
        Some(carnet::app::PendingMutationKind::File(_))
    ));
}

#[test]
fn discarding_dirty_ancestor_rename_abandons_edits_and_blocks_replacement_races() {
    use carnet::{
        app::{Dialog, DirtyChoice, EffectExecutor, Focus, TreeAction},
        git::CommitOutcome,
        workspace::FileOperation,
    };

    let (_sandbox, mut app) = app_with_note(85, "folder/note.md", "base");
    app.update(AppEvent::Action(AppAction::Editor(EditorCommand::Insert(
        "dirty ".into(),
    ))));
    app.update(AppEvent::Action(AppAction::Focus(Focus::Tree)));
    app.update(AppEvent::Action(AppAction::Tree(TreeAction::Rename)));
    assert!(
        app.update(AppEvent::Action(AppAction::SubmitFileAction(
            PathBuf::from("renamed"),
        )))
        .is_empty()
    );
    assert_eq!(app.dialog, Some(Dialog::DirtyNavigation));

    let rename = app.update(AppEvent::DirtyChoice(DirtyChoice::Discard));
    assert!(matches!(
        mutation_parts(&rename[0]).1,
        FileOperation::Rename { from, to, .. }
            if from == PathBuf::from("folder").as_path()
                && to == PathBuf::from("renamed").as_path()
    ));
    assert_eq!(workspace_editor(&app).text(), "base");
    assert!(!workspace_editor(&app).is_dirty());

    assert!(
        app.update(AppEvent::Action(AppAction::Editor(EditorCommand::Insert(
            "blocked during mutation".into(),
        ))))
        .is_empty()
    );
    assert_eq!(workspace_editor(&app).text(), "base");

    let (mutation_id, repository_id, repository_root, file, tree) =
        apply_mutation_effect(rename.into_iter().next().unwrap());
    let load = app.update(AppEvent::MutationApplied {
        mutation_id,
        repository_id,
        repository_root,
        file,
        commit: CommitOutcome::NoChanges,
        tree,
    });
    assert!(matches!(&load[..], [AppEffect::LoadNote { .. }]));

    app.update(AppEvent::Action(AppAction::Editor(EditorCommand::Insert(
        "blocked during load".into(),
    ))));
    assert_eq!(workspace_editor(&app).text(), "base");
    let loaded = EffectExecutor::default()
        .execute(load.into_iter().next().unwrap())
        .unwrap();
    app.update(loaded);
    assert_eq!(workspace_editor(&app).text(), "base");
    let Screen::Workspace(workspace) = &app.screen else {
        panic!("expected workspace");
    };
    assert_eq!(
        workspace.current_note.as_deref(),
        Some(PathBuf::from("renamed/note.md").as_path())
    );
}

#[test]
fn cancelling_a_dirty_ancestor_move_keeps_the_note_and_performs_no_mutation() {
    use carnet::app::{Dialog, DirtyChoice, FileMutationAction, Focus, TreeAction};

    let (sandbox, mut app) = app_with_note(86, "folder/note.md", "base");
    fs::create_dir(sandbox.path().join("archive")).unwrap();
    app.update(AppEvent::Action(AppAction::Editor(EditorCommand::Insert(
        "dirty ".into(),
    ))));
    app.update(AppEvent::Action(AppAction::Focus(Focus::Tree)));
    app.update(AppEvent::Action(AppAction::Tree(TreeAction::Move)));

    assert!(
        app.update(AppEvent::Action(AppAction::SubmitFileAction(
            PathBuf::from("archive/folder"),
        )))
        .is_empty()
    );
    assert_eq!(app.dialog, Some(Dialog::DirtyNavigation));
    assert!(matches!(
        app.pending_intent.as_ref(),
        Some(PendingIntent::Mutation(PendingFileMutation {
            action: FileMutationAction::Move { from, to },
            ..
        })) if from == PathBuf::from("folder").as_path()
            && to == PathBuf::from("archive/folder").as_path()
    ));

    assert!(
        app.update(AppEvent::DirtyChoice(DirtyChoice::Cancel))
            .is_empty()
    );
    assert_eq!(app.pending_intent, None);
    assert_eq!(app.dialog, None);
    assert_eq!(workspace_editor(&app).text(), "dirty base");
    assert!(workspace_editor(&app).is_dirty());
    assert!(sandbox.path().join("folder/note.md").is_file());
    assert!(!sandbox.path().join("archive/folder").exists());
}

#[test]
fn dirty_ancestor_delete_waits_for_save_before_deleting_the_tree() {
    use carnet::{
        app::{Dialog, DirtyChoice, FileMutationAction, Focus, TreeAction},
        git::{CommitIntent, CommitOutcome},
        workspace::FileOperation,
    };

    let (_sandbox, mut app) = app_with_note(87, "folder/note.md", "base");
    app.update(AppEvent::Action(AppAction::Editor(EditorCommand::Insert(
        "dirty ".into(),
    ))));
    app.update(AppEvent::Action(AppAction::Focus(Focus::Tree)));
    app.update(AppEvent::Action(AppAction::Tree(TreeAction::Delete)));

    assert!(
        app.update(AppEvent::Action(AppAction::ConfirmDelete))
            .is_empty()
    );
    assert_eq!(app.dialog, Some(Dialog::DirtyNavigation));
    assert!(matches!(
        app.pending_intent.as_ref(),
        Some(PendingIntent::Mutation(PendingFileMutation {
            action: FileMutationAction::Delete { path },
            ..
        })) if path == PathBuf::from("folder").as_path()
    ));

    let save = app.update(AppEvent::DirtyChoice(DirtyChoice::Save));
    let (mutation_id, repository_id, repository_root, file) =
        apply_save_effect(save.into_iter().next().unwrap());
    let delete = app.update(AppEvent::MutationApplied {
        mutation_id,
        repository_id,
        repository_root,
        file,
        commit: CommitOutcome::NoChanges,
        tree: Ok(Vec::new()),
    });
    assert!(matches!(
        mutation_parts(&delete[0]),
        (
            _,
            FileOperation::Delete {
                path,
                confirmed: true,
                ..
            },
            CommitIntent::Delete(intent_path),
        ) if path == PathBuf::from("folder").as_path()
            && intent_path == PathBuf::from("folder").as_path()
    ));
    assert!(!workspace_editor(&app).is_dirty());
}

#[test]
fn dirty_unrelated_rename_does_not_replace_or_block_the_active_editor() {
    use carnet::{
        app::{Focus, TreeAction},
        git::CommitOutcome,
    };

    let (sandbox, repository, workspace, git) =
        workspace_fixture(88, "notes", "active/note.md", "base");
    fs::create_dir(repository.path.join("unrelated")).unwrap();
    let tree = workspace.tree().unwrap();
    let note = workspace
        .load_note(
            &workspace
                .resolve_note(PathBuf::from("active/note.md").as_path())
                .unwrap(),
        )
        .unwrap();
    let mut app = App::home(vec![repository.clone()], Some(repository.id), None);
    app.update(AppEvent::Action(AppAction::Home(HomeAction::OpenSelected)));
    let request_id = app.pending_request.as_ref().unwrap().request_id();
    app.update(AppEvent::WorkspaceOpened {
        request_id,
        repository_id: repository.id,
        workspace,
        git,
        tree,
        note: Some(note),
    });

    app.update(AppEvent::Action(AppAction::Editor(EditorCommand::Insert(
        "dirty ".into(),
    ))));
    app.update(AppEvent::Action(AppAction::Focus(Focus::Tree)));
    app.update(AppEvent::Action(AppAction::Tree(TreeAction::Down)));
    app.update(AppEvent::Action(AppAction::Tree(TreeAction::Rename)));
    let rename = app.update(AppEvent::Action(AppAction::SubmitFileAction(
        PathBuf::from("renamed-unrelated"),
    )));
    assert!(matches!(&rename[..], [AppEffect::ApplyAndCommit { .. }]));

    let (mutation_id, repository_id, repository_root, file, tree) =
        apply_mutation_effect(rename.into_iter().next().unwrap());
    assert!(
        app.update(AppEvent::MutationApplied {
            mutation_id,
            repository_id,
            repository_root,
            file,
            commit: CommitOutcome::NoChanges,
            tree,
        })
        .is_empty()
    );
    assert_eq!(workspace_editor(&app).text(), "dirty base");
    assert!(workspace_editor(&app).is_dirty());
    let Screen::Workspace(workspace) = &app.screen else {
        panic!("expected workspace");
    };
    assert_eq!(
        workspace.current_note.as_deref(),
        Some(PathBuf::from("active/note.md").as_path())
    );
    assert!(!sandbox.path().join("unrelated").exists());
    assert!(sandbox.path().join("renamed-unrelated").is_dir());
}

#[test]
fn pending_load_blocks_unrelated_and_active_mutations_until_the_load_resolves() {
    use carnet::{
        app::{EffectExecutor, Focus, NavigationAction, TreeAction},
        workspace::FileOperation,
    };

    let (_sandbox, mut unrelated) = app_with_note(90, "note.md", "base");
    let unrelated_load = unrelated
        .update(AppEvent::Action(AppAction::Navigate(
            NavigationAction::Note(PathBuf::from("note.md")),
        )))
        .pop()
        .unwrap();
    unrelated.update(AppEvent::Action(AppAction::Focus(Focus::Tree)));
    unrelated.update(AppEvent::Action(AppAction::Tree(TreeAction::NewFolder)));
    assert!(
        unrelated
            .update(AppEvent::Action(AppAction::SubmitFileAction(
                PathBuf::from("folder"),
            )))
            .is_empty()
    );
    assert_eq!(unrelated.dialog, None);

    let (_sandbox, mut active) = app_with_note(91, "note.md", "base");
    let active_load = active
        .update(AppEvent::Action(AppAction::Navigate(
            NavigationAction::Note(PathBuf::from("note.md")),
        )))
        .pop()
        .unwrap();
    active.update(AppEvent::Action(AppAction::Focus(Focus::Tree)));
    active.update(AppEvent::Action(AppAction::Tree(TreeAction::Rename)));
    assert!(
        active
            .update(AppEvent::Action(AppAction::SubmitFileAction(
                PathBuf::from("renamed.md"),
            )))
            .is_empty()
    );
    assert_eq!(active.dialog, None);

    unrelated.update(EffectExecutor::default().execute(unrelated_load).unwrap());
    active.update(EffectExecutor::default().execute(active_load).unwrap());
    active.update(AppEvent::Action(AppAction::Tree(TreeAction::Rename)));
    let rename = active.update(AppEvent::Action(AppAction::SubmitFileAction(
        PathBuf::from("renamed.md"),
    )));
    assert!(matches!(
        mutation_parts(&rename[0]).1,
        FileOperation::Rename { from, to, .. }
            if from == PathBuf::from("note.md").as_path()
                && to == PathBuf::from("renamed.md").as_path()
    ));
}

#[test]
fn pending_workspace_open_blocks_mutation_start_in_the_old_visible_repository() {
    use carnet::app::{Focus, NavigationAction, TreeAction};

    let (_sandbox, mut app) = app_with_note(92, "note.md", "base");
    let repository_b = repository(93, "repository-b");
    let open = app.update(AppEvent::Action(AppAction::Navigate(
        NavigationAction::Repository {
            repository: repository_b,
            note: None,
        },
    )));
    assert!(matches!(&open[..], [AppEffect::OpenWorkspace { .. }]));

    app.update(AppEvent::Action(AppAction::Focus(Focus::Tree)));
    app.update(AppEvent::Action(AppAction::Tree(TreeAction::NewFolder)));
    assert!(
        app.update(AppEvent::Action(AppAction::SubmitFileAction(
            PathBuf::from("must-wait"),
        )))
        .is_empty()
    );
    assert_eq!(app.dialog, None);
    assert!(app.pending_mutation.is_none());
}

#[test]
fn workspace_open_invalidates_a_rename_dialog_from_the_previous_repository() {
    use carnet::app::{
        Dialog, EffectExecutor, FileActionKind, Focus, NavigationAction, TreeAction,
    };

    let (_sandbox_a, mut app) = app_with_note(100, "note.md", "a");
    let (sandbox_b, repository_b, _workspace_b, _git_b) =
        workspace_fixture(101, "repository-b", "note.md", "b");
    app.home.repositories.push(repository_b.clone());
    app.home
        .repository_availability
        .push(carnet::app::RepositoryAvailability::Available);
    app.update(AppEvent::Action(AppAction::Focus(Focus::Tree)));
    app.update(AppEvent::Action(AppAction::Tree(TreeAction::Rename)));
    assert!(matches!(
        app.dialog,
        Some(Dialog::FileAction {
            kind: FileActionKind::Rename,
            ..
        })
    ));

    let open = app
        .update(AppEvent::Action(AppAction::Navigate(
            NavigationAction::Repository {
                repository: repository_b.clone(),
                note: None,
            },
        )))
        .pop()
        .unwrap();
    assert!(
        app.update(AppEvent::Action(AppAction::SubmitFileAction(
            PathBuf::from("renamed.md"),
        )))
        .is_empty()
    );
    assert!(app.dialog.is_some());

    app.update(EffectExecutor::default().execute(open).unwrap());
    let Screen::Workspace(workspace) = &app.screen else {
        panic!("expected repository B workspace");
    };
    assert_eq!(workspace.repository.id, repository_b.id);
    assert_eq!(app.dialog, None);
    assert!(
        app.update(AppEvent::Action(AppAction::SubmitFileAction(
            PathBuf::from("renamed.md"),
        )))
        .is_empty()
    );
    assert!(app.pending_mutation.is_none());
    assert!(sandbox_b.path().join("note.md").is_file());
    assert!(!sandbox_b.path().join("renamed.md").exists());
}

#[test]
fn workspace_open_invalidates_delete_confirmation_from_the_previous_repository() {
    use carnet::app::{Dialog, EffectExecutor, Focus, NavigationAction, TreeAction};

    let (_sandbox_a, mut app) = app_with_note(102, "note.md", "a");
    let (sandbox_b, repository_b, _workspace_b, _git_b) =
        workspace_fixture(103, "repository-b", "note.md", "b");
    app.home.repositories.push(repository_b.clone());
    app.home
        .repository_availability
        .push(carnet::app::RepositoryAvailability::Available);
    app.update(AppEvent::Action(AppAction::Focus(Focus::Tree)));
    app.update(AppEvent::Action(AppAction::Tree(TreeAction::Delete)));
    assert!(matches!(app.dialog, Some(Dialog::ConfirmDelete { .. })));

    let open = app
        .update(AppEvent::Action(AppAction::Navigate(
            NavigationAction::Repository {
                repository: repository_b.clone(),
                note: None,
            },
        )))
        .pop()
        .unwrap();
    assert!(
        app.update(AppEvent::Action(AppAction::ConfirmDelete))
            .is_empty()
    );
    assert!(app.dialog.is_some());

    app.update(EffectExecutor::default().execute(open).unwrap());
    assert_eq!(app.dialog, None);
    assert!(
        app.update(AppEvent::Action(AppAction::ConfirmDelete))
            .is_empty()
    );
    assert!(app.pending_mutation.is_none());
    assert!(sandbox_b.path().join("note.md").is_file());
}

#[test]
fn forged_dialog_origins_are_rejected_at_submit_and_confirm() {
    use carnet::app::{Dialog, Focus, TreeAction};

    let (rename_sandbox, mut rename) = app_with_note(104, "note.md", "base");
    rename.update(AppEvent::Action(AppAction::Focus(Focus::Tree)));
    rename.update(AppEvent::Action(AppAction::Tree(TreeAction::Rename)));
    let Some(Dialog::FileAction { origin, .. }) = &mut rename.dialog else {
        panic!("expected rename dialog");
    };
    origin.repository_id = Uuid::from_u128(999);

    assert!(
        rename
            .update(AppEvent::Action(AppAction::SubmitFileAction(
                PathBuf::from("renamed.md"),
            )))
            .is_empty()
    );
    assert_eq!(rename.dialog, None);
    assert!(rename.pending_mutation.is_none());
    assert!(rename_sandbox.path().join("note.md").is_file());
    assert!(!rename_sandbox.path().join("renamed.md").exists());

    let (delete_sandbox, mut delete) = app_with_note(105, "note.md", "base");
    delete.update(AppEvent::Action(AppAction::Focus(Focus::Tree)));
    delete.update(AppEvent::Action(AppAction::Tree(TreeAction::Delete)));
    let Some(Dialog::ConfirmDelete { origin, .. }) = &mut delete.dialog else {
        panic!("expected delete confirmation");
    };
    origin.repository_root = PathBuf::from("/forged/root");

    assert!(
        delete
            .update(AppEvent::Action(AppAction::ConfirmDelete))
            .is_empty()
    );
    assert_eq!(delete.dialog, None);
    assert!(delete.pending_mutation.is_none());
    assert!(delete_sandbox.path().join("note.md").is_file());
}

#[test]
fn same_workspace_load_retains_dialog_origin_and_original_rename_target() {
    use carnet::{
        app::{Dialog, EffectExecutor, FileActionKind, Focus, NavigationAction, TreeAction},
        workspace::FileOperation,
    };

    let (_sandbox, mut app) = app_with_note(106, "folder/note.md", "base");
    app.update(AppEvent::Action(AppAction::Focus(Focus::Tree)));
    app.update(AppEvent::Action(AppAction::Tree(TreeAction::Rename)));
    let Some(Dialog::FileAction {
        origin,
        kind: FileActionKind::Rename,
        target: Some(target),
    }) = app.dialog.clone()
    else {
        panic!("expected scoped rename dialog");
    };
    assert_eq!(target, PathBuf::from("folder"));

    let load = app
        .update(AppEvent::Action(AppAction::Navigate(
            NavigationAction::Note(PathBuf::from("folder/note.md")),
        )))
        .pop()
        .unwrap();
    assert!(
        app.update(AppEvent::Action(AppAction::SubmitFileAction(
            PathBuf::from("renamed"),
        )))
        .is_empty()
    );
    assert!(app.dialog.is_some());
    app.update(EffectExecutor::default().execute(load).unwrap());

    assert_eq!(
        app.dialog,
        Some(Dialog::FileAction {
            origin: origin.clone(),
            kind: FileActionKind::Rename,
            target: Some(PathBuf::from("folder")),
        })
    );
    let rename = app.update(AppEvent::Action(AppAction::SubmitFileAction(
        PathBuf::from("renamed"),
    )));
    assert!(matches!(
        mutation_parts(&rename[0]).1,
        FileOperation::Rename { from, to, .. }
            if from == PathBuf::from("folder").as_path()
                && to == PathBuf::from("renamed").as_path()
    ));
    let (_, repository_id, repository_root) = mutation_identity(&rename[0]);
    assert_eq!(repository_id, origin.repository_id);
    assert_eq!(repository_root, origin.repository_root);
}

#[test]
fn workspace_open_clears_old_deferred_file_intent_but_keeps_global_dialog() {
    use carnet::app::{
        Dialog, EffectExecutor, FailureKind, FileMutationAction, Focus, NavigationAction,
        TreeAction,
    };

    let (_sandbox_a, mut app) = app_with_note(107, "note.md", "a");
    let (_sandbox_b, repository_b, _workspace_b, _git_b) =
        workspace_fixture(108, "repository-b", "note.md", "b");
    app.home.repositories.push(repository_b.clone());
    app.home
        .repository_availability
        .push(carnet::app::RepositoryAvailability::Available);
    app.update(AppEvent::Action(AppAction::Focus(Focus::Tree)));
    app.update(AppEvent::Action(AppAction::Tree(TreeAction::Rename)));
    let Some(Dialog::FileAction { origin, .. }) = app.dialog.clone() else {
        panic!("expected repository A dialog origin");
    };
    let open = app
        .update(AppEvent::Action(AppAction::Navigate(
            NavigationAction::Repository {
                repository: repository_b,
                note: None,
            },
        )))
        .pop()
        .unwrap();

    app.pending_intent = Some(PendingIntent::Mutation(PendingFileMutation {
        origin,
        action: FileMutationAction::Rename {
            from: PathBuf::from("note.md"),
            to: PathBuf::from("renamed.md"),
        },
    }));
    app.dialog = Some(Dialog::Failure {
        kind: FailureKind::Runtime,
        message: "global failure".into(),
    });
    app.update(EffectExecutor::default().execute(open).unwrap());

    assert_eq!(app.pending_intent, None);
    assert_eq!(
        app.dialog,
        Some(Dialog::Failure {
            kind: FailureKind::Runtime,
            message: "global failure".into(),
        })
    );
}

#[test]
fn navigation_cannot_overwrite_a_deferred_dirty_mutation_intent() {
    use carnet::app::{Dialog, Focus, NavigationAction, TreeAction};

    let (_sandbox, mut app) = app_with_note(99, "note.md", "base");
    app.update(AppEvent::Action(AppAction::Editor(EditorCommand::Insert(
        "dirty ".into(),
    ))));
    app.update(AppEvent::Action(AppAction::Focus(Focus::Tree)));
    app.update(AppEvent::Action(AppAction::Tree(TreeAction::NewFile)));
    app.update(AppEvent::Action(AppAction::SubmitFileAction(
        PathBuf::from("created.md"),
    )));
    let deferred = app.pending_intent.clone().unwrap();

    assert!(
        app.update(AppEvent::Action(AppAction::Navigate(
            NavigationAction::Note(PathBuf::from("other.md")),
        )))
        .is_empty()
    );
    assert_eq!(app.pending_intent, Some(deferred));
    assert_eq!(app.dialog, Some(Dialog::DirtyNavigation));
    assert!(app.pending_request.is_none());
}

#[test]
fn stale_mutation_success_cannot_clear_a_newer_same_repository_mutation() {
    use carnet::{
        app::{FileActionKind, Focus, PendingMutationKind, TreeAction},
        git::{CommitOutcome, MutationCommitError},
    };

    let (_sandbox, mut app) = empty_app(94);
    app.update(AppEvent::Action(AppAction::Focus(Focus::Tree)));
    app.update(AppEvent::Action(AppAction::Tree(TreeAction::NewFolder)));
    let stale = app
        .update(AppEvent::Action(AppAction::SubmitFileAction(
            PathBuf::from("stale"),
        )))
        .pop()
        .unwrap();
    let (mutation_id, repository_id, repository_root) = mutation_identity(&stale);
    app.update(AppEvent::MutationFailed {
        mutation_id,
        repository_id,
        repository_root,
        error: MutationCommitError::WorkspaceMismatch,
    });

    app.update(AppEvent::Action(AppAction::Tree(TreeAction::NewFolder)));
    let current = app.update(AppEvent::Action(AppAction::SubmitFileAction(
        PathBuf::from("current"),
    )));
    assert!(matches!(&current[..], [AppEffect::ApplyAndCommit { .. }]));
    let current_identity = mutation_identity(&current[0]);
    assert!(mutation_id.get() < current_identity.0.get());
    let (mutation_id, repository_id, repository_root, file, tree) = apply_mutation_effect(stale);

    assert!(
        app.update(AppEvent::MutationApplied {
            mutation_id,
            repository_id,
            repository_root,
            file,
            commit: CommitOutcome::NoChanges,
            tree,
        })
        .is_empty()
    );
    assert_eq!(pending_mutation_identity(&app), current_identity);
    assert!(matches!(
        app.pending_mutation.as_ref().map(|pending| pending.kind),
        Some(PendingMutationKind::File(FileActionKind::NewFolder))
    ));
    let Screen::Workspace(workspace) = &app.screen else {
        panic!("expected workspace");
    };
    assert!(workspace.tree.is_empty());
    assert_eq!(workspace.tree_selection, None);
}

#[test]
fn repository_a_mutation_success_cannot_touch_repository_b_state() {
    use carnet::{
        app::{Focus, TreeAction},
        git::CommitOutcome,
    };

    let (_sandbox_a, mut app_a) = empty_app(95);
    app_a.update(AppEvent::Action(AppAction::Focus(Focus::Tree)));
    app_a.update(AppEvent::Action(AppAction::Tree(TreeAction::NewFolder)));
    let stale = app_a
        .update(AppEvent::Action(AppAction::SubmitFileAction(
            PathBuf::from("from-a"),
        )))
        .pop()
        .unwrap();
    let (mutation_id, repository_id, repository_root, file, tree) = apply_mutation_effect(stale);

    let (_sandbox_b, mut app_b) = empty_app(96);
    app_b.update(AppEvent::Action(AppAction::Focus(Focus::Tree)));
    app_b.update(AppEvent::Action(AppAction::Tree(TreeAction::NewFolder)));
    let current = app_b.update(AppEvent::Action(AppAction::SubmitFileAction(
        PathBuf::from("from-b"),
    )));
    let (current_mutation_id, current_repository_id, current_root) = mutation_identity(&current[0]);
    assert_eq!(mutation_id, current_mutation_id);

    assert!(
        app_b
            .update(AppEvent::MutationApplied {
                mutation_id,
                repository_id,
                repository_root,
                file,
                commit: CommitOutcome::NoChanges,
                tree,
            })
            .is_empty()
    );
    assert_eq!(
        pending_mutation_identity(&app_b),
        (current_mutation_id, current_repository_id, current_root)
    );
    let Screen::Workspace(workspace) = &app_b.screen else {
        panic!("expected repository B workspace");
    };
    assert_eq!(workspace.repository.id, Uuid::from_u128(96));
    assert!(workspace.tree.is_empty());
    assert_eq!(workspace.tree_selection, None);
}

#[test]
fn stale_failure_and_conflict_cannot_replace_the_current_mutation() {
    use carnet::{
        app::{CommitStatus, ExternalConflict, Focus, TreeAction},
        git::MutationCommitError,
    };

    let (_sandbox, mut app) = empty_app(97);
    app.update(AppEvent::Action(AppAction::Focus(Focus::Tree)));
    app.update(AppEvent::Action(AppAction::Tree(TreeAction::NewFolder)));
    let stale = app
        .update(AppEvent::Action(AppAction::SubmitFileAction(
            PathBuf::from("stale"),
        )))
        .pop()
        .unwrap();
    let (stale_id, repository_id, repository_root) = mutation_identity(&stale);
    app.update(AppEvent::MutationFailed {
        mutation_id: stale_id,
        repository_id,
        repository_root: repository_root.clone(),
        error: MutationCommitError::WorkspaceMismatch,
    });
    app.update(AppEvent::Action(AppAction::Dismiss));

    app.update(AppEvent::Action(AppAction::Tree(TreeAction::NewFolder)));
    let current = app.update(AppEvent::Action(AppAction::SubmitFileAction(
        PathBuf::from("current"),
    )));
    let current_identity = mutation_identity(&current[0]);
    let recorded_failure_count = app.failures.runtime.len();

    app.update(AppEvent::MutationFailed {
        mutation_id: stale_id,
        repository_id,
        repository_root: repository_root.clone(),
        error: MutationCommitError::RepositoryMismatch,
    });
    app.update(AppEvent::MutationConflict {
        mutation_id: stale_id,
        repository_id,
        repository_root,
        conflict: ExternalConflict::Modified {
            path: PathBuf::from("stale"),
        },
    });

    assert_eq!(pending_mutation_identity(&app), current_identity);
    assert_eq!(app.status.commit, CommitStatus::Pending);
    assert_eq!(app.dialog, None);
    assert_eq!(app.failures.runtime.len(), recorded_failure_count);
}

#[test]
fn stale_saved_commit_failure_cannot_mark_a_newer_save_clean_or_failed() {
    use carnet::{
        app::{CommitStatus, GlobalAction},
        git::{GitError, MutationCommitError},
    };

    let (_sandbox, mut app) = app_with_note(98, "note.md", "base");
    app.update(AppEvent::Action(AppAction::Editor(EditorCommand::Insert(
        "dirty ".into(),
    ))));
    let stale = app.update(AppEvent::Action(AppAction::Global(GlobalAction::Save)));
    let (stale_id, repository_id, repository_root, file) =
        apply_save_effect(stale.into_iter().next().unwrap());
    app.update(AppEvent::MutationFailed {
        mutation_id: stale_id,
        repository_id,
        repository_root: repository_root.clone(),
        error: MutationCommitError::WorkspaceMismatch,
    });
    app.update(AppEvent::Action(AppAction::Dismiss));

    let current = app.update(AppEvent::Action(AppAction::Global(GlobalAction::Save)));
    let current_identity = mutation_identity(&current[0]);
    app.update(AppEvent::MutationSavedCommitFailed {
        mutation_id: stale_id,
        repository_id,
        repository_root,
        file,
        error: GitError::CommandFailed {
            operation: "commit",
            status: Some(1),
            stderr: "stale failure".into(),
        },
        tree: Ok(Vec::new()),
    });

    assert_eq!(pending_mutation_identity(&app), current_identity);
    assert!(workspace_editor(&app).is_dirty());
    assert_eq!(app.saved_commit_failure, None);
    assert_eq!(app.failures.git, None);
    assert_eq!(app.status.commit, CommitStatus::Pending);
    assert_eq!(app.dialog, None);
}

#[test]
fn edits_after_save_start_keep_newer_text_dirty_when_save_completes() {
    use carnet::{app::GlobalAction, git::CommitOutcome};

    let (_sandbox, mut app) = app_with_note(72, "note.md", "base");
    app.update(AppEvent::Action(AppAction::Editor(EditorCommand::Insert(
        "saved ".into(),
    ))));
    let save = app.update(AppEvent::Action(AppAction::Global(GlobalAction::Save)));
    app.update(AppEvent::Action(AppAction::Editor(EditorCommand::Insert(
        "newer ".into(),
    ))));
    let (mutation_id, repository_id, repository_root, file) =
        apply_save_effect(save.into_iter().next().unwrap());

    app.update(AppEvent::MutationApplied {
        mutation_id,
        repository_id,
        repository_root,
        file,
        commit: CommitOutcome::NoChanges,
        tree: Ok(Vec::new()),
    });

    assert_eq!(workspace_editor(&app).text(), "saved newer base");
    assert!(workspace_editor(&app).is_dirty());
    let Screen::Workspace(workspace) = &app.screen else {
        panic!("expected workspace");
    };
    assert_eq!(
        fs::read_to_string(workspace.workspace.root().join("note.md")).unwrap(),
        "saved base"
    );
}

#[test]
fn dirty_navigation_does_not_resume_when_edits_arrive_after_save_start() {
    use carnet::{
        app::{Dialog, DirtyChoice, NavigationAction},
        git::CommitOutcome,
    };

    let (_sandbox, mut app) = app_with_note(73, "note.md", "base");
    app.update(AppEvent::Action(AppAction::Editor(EditorCommand::Insert(
        "saved ".into(),
    ))));
    app.update(AppEvent::Action(AppAction::Navigate(
        NavigationAction::Quit,
    )));
    let save = app.update(AppEvent::DirtyChoice(DirtyChoice::Save));
    app.update(AppEvent::Action(AppAction::Editor(EditorCommand::Insert(
        "newer ".into(),
    ))));
    let (mutation_id, repository_id, repository_root, file) =
        apply_save_effect(save.into_iter().next().unwrap());

    let effects = app.update(AppEvent::MutationApplied {
        mutation_id,
        repository_id,
        repository_root,
        file,
        commit: CommitOutcome::NoChanges,
        tree: Ok(Vec::new()),
    });

    assert!(effects.is_empty());
    assert!(!app.quit.requested);
    assert_eq!(
        app.pending_intent,
        Some(PendingIntent::Navigation(NavigationAction::Quit))
    );
    assert!(matches!(app.dialog, Some(Dialog::DirtyNavigation)));
    assert!(workspace_editor(&app).is_dirty());
}

#[test]
fn dirty_navigation_cancel_clears_the_exact_pending_action() {
    use carnet::app::{Dialog, DirtyChoice, NavigationAction};

    let (_sandbox, mut app) = app_with_note(23, "note.md", "base");
    app.update(AppEvent::Action(AppAction::Editor(EditorCommand::Insert(
        "dirty ".into(),
    ))));
    let target = NavigationAction::Note(PathBuf::from("other.md"));

    let effects = app.update(AppEvent::Action(AppAction::Navigate(target.clone())));

    assert!(effects.is_empty());
    assert_eq!(app.pending_intent, Some(PendingIntent::Navigation(target)));
    assert!(matches!(app.dialog, Some(Dialog::DirtyNavigation)));

    assert!(
        app.update(AppEvent::DirtyChoice(DirtyChoice::Cancel))
            .is_empty()
    );
    assert_eq!(app.pending_intent, None);
    assert_eq!(app.dialog, None);
    assert_eq!(workspace_editor(&app).text(), "dirty base");
}

#[test]
fn dirty_navigation_discard_resumes_the_pending_note_immediately() {
    use carnet::app::{DirtyChoice, NavigationAction};

    let (_sandbox, mut app) = app_with_note(24, "note.md", "base");
    app.update(AppEvent::Action(AppAction::Editor(EditorCommand::Insert(
        "dirty ".into(),
    ))));
    let target = NavigationAction::Note(PathBuf::from("other.md"));
    app.update(AppEvent::Action(AppAction::Navigate(target)));

    let effects = app.update(AppEvent::DirtyChoice(DirtyChoice::Discard));

    assert_eq!(app.pending_intent, None);
    assert_eq!(app.dialog, None);
    assert_eq!(
        app.pending_request
            .as_ref()
            .and_then(|pending| pending.path()),
        Some(PathBuf::from("other.md").as_path())
    );
    assert!(matches!(
        &effects[..],
        [AppEffect::LoadNote {
            repository_id,
            path,
            ..
        }] if *repository_id == Uuid::from_u128(24) && path == PathBuf::from("other.md").as_path()
    ));
}

#[test]
fn navigation_models_home_and_exact_cross_repository_targets() {
    use carnet::app::{DirtyChoice, NavigationAction};

    let (_clean_sandbox, mut clean) = app_with_note(45, "note.md", "base");
    clean.update(AppEvent::Action(AppAction::Navigate(
        NavigationAction::Home,
    )));
    assert!(matches!(clean.screen, Screen::Home));

    let (_dirty_sandbox, mut dirty) = app_with_note(46, "note.md", "base");
    dirty.update(AppEvent::Action(AppAction::Editor(EditorCommand::Insert(
        "dirty ".into(),
    ))));
    let other = repository(47, "other");
    let target = NavigationAction::Repository {
        repository: other.clone(),
        note: Some(PathBuf::from("target.md")),
    };
    dirty.update(AppEvent::Action(AppAction::Navigate(target.clone())));
    assert_eq!(
        dirty.pending_intent,
        Some(PendingIntent::Navigation(target))
    );

    let effects = dirty.update(AppEvent::DirtyChoice(DirtyChoice::Discard));
    assert!(matches!(
        &effects[..],
        [AppEffect::OpenWorkspace {
            repository: opened,
            note: Some(path),
            ..
        }] if opened == &other && path == PathBuf::from("target.md").as_path()
    ));
}

#[test]
fn dirty_navigation_save_resumes_only_after_the_confirmed_save_result() {
    use carnet::{
        app::{CommitStatus, DirtyChoice, NavigationAction},
        git::CommitOutcome,
    };

    let (_sandbox, mut app) = app_with_note(25, "note.md", "base");
    app.update(AppEvent::Action(AppAction::Editor(EditorCommand::Insert(
        "dirty ".into(),
    ))));
    let target = NavigationAction::Note(PathBuf::from("other.md"));
    app.update(AppEvent::Action(AppAction::Navigate(target.clone())));

    let save_effects = app.update(AppEvent::DirtyChoice(DirtyChoice::Save));

    assert_eq!(app.pending_intent, Some(PendingIntent::Navigation(target)));
    assert_eq!(app.dialog, None);
    assert_eq!(save_effects.len(), 1);
    let (mutation_id, repository_id, repository_root, file) =
        apply_save_effect(save_effects.into_iter().next().unwrap());

    let resumed = app.update(AppEvent::MutationApplied {
        mutation_id,
        repository_id,
        repository_root,
        file,
        commit: CommitOutcome::NoChanges,
        tree: Ok(Vec::new()),
    });

    assert_eq!(app.pending_mutation, None);
    assert_eq!(app.pending_intent, None);
    assert_eq!(app.status.commit, CommitStatus::NoChanges);
    assert!(!workspace_editor(&app).is_dirty());
    assert!(matches!(
        &resumed[..],
        [AppEffect::LoadNote { path, .. }] if path == PathBuf::from("other.md").as_path()
    ));
}

#[test]
fn global_quit_is_immediate_when_clean_and_uses_the_dirty_prompt_when_changed() {
    use carnet::app::{AppExitStatus, Dialog, DirtyChoice, GlobalAction, NavigationAction};

    let (_clean_sandbox, mut clean) = app_with_note(26, "note.md", "base");
    assert!(
        clean
            .update(AppEvent::Action(AppAction::Global(GlobalAction::Quit)))
            .is_empty()
    );
    assert!(clean.quit.requested);
    assert_eq!(clean.quit.final_status, Some(AppExitStatus::Success));

    let (_dirty_sandbox, mut dirty) = app_with_note(27, "note.md", "base");
    dirty.update(AppEvent::Action(AppAction::Editor(EditorCommand::Insert(
        "x".into(),
    ))));
    dirty.update(AppEvent::Action(AppAction::Global(GlobalAction::Quit)));
    assert!(!dirty.quit.requested);
    assert_eq!(
        dirty.pending_intent,
        Some(PendingIntent::Navigation(NavigationAction::Quit))
    );
    assert!(matches!(dirty.dialog, Some(Dialog::DirtyNavigation)));

    dirty.update(AppEvent::DirtyChoice(DirtyChoice::Discard));
    assert!(dirty.quit.requested);
    assert_eq!(dirty.quit.final_status, Some(AppExitStatus::Success));
}

#[test]
fn cancelling_an_external_conflict_keeps_the_dirty_buffer() {
    use carnet::app::{ConflictChoice, Dialog, ExternalConflict, GlobalAction};

    let (_sandbox, mut app) = app_with_note(28, "note.md", "base");
    app.update(AppEvent::Action(AppAction::Editor(EditorCommand::Insert(
        "mine ".into(),
    ))));
    app.update(AppEvent::Action(AppAction::Global(GlobalAction::Save)));
    let (mutation_id, _, repository_root) = pending_mutation_identity(&app);
    let conflict = ExternalConflict::Modified {
        path: PathBuf::from("note.md"),
    };

    app.update(AppEvent::MutationConflict {
        mutation_id,
        repository_id: Uuid::from_u128(28),
        repository_root,
        conflict: conflict.clone(),
    });

    assert_eq!(app.pending_mutation, None);
    assert_eq!(app.dialog, Some(Dialog::ExternalConflict(conflict)));
    assert!(workspace_editor(&app).is_dirty());

    app.update(AppEvent::ConflictChoice(ConflictChoice::Cancel));
    assert_eq!(app.dialog, None);
    assert!(workspace_editor(&app).is_dirty());
}

#[test]
fn overwriting_an_external_conflict_emits_an_overwrite_save() {
    use carnet::{
        app::{ConflictChoice, ExternalConflict, GlobalAction, PendingMutationKind},
        workspace::FileOperation,
    };

    let (_sandbox, mut app) = app_with_note(29, "note.md", "base");
    app.update(AppEvent::Action(AppAction::Editor(EditorCommand::Insert(
        "mine ".into(),
    ))));
    app.update(AppEvent::Action(AppAction::Global(GlobalAction::Save)));
    let (mutation_id, _, repository_root) = pending_mutation_identity(&app);
    app.update(AppEvent::MutationConflict {
        mutation_id,
        repository_id: Uuid::from_u128(29),
        repository_root,
        conflict: ExternalConflict::Modified {
            path: PathBuf::from("note.md"),
        },
    });

    let effects = app.update(AppEvent::ConflictChoice(ConflictChoice::Overwrite));

    assert_eq!(app.dialog, None);
    assert!(matches!(
        app.pending_mutation.as_ref().map(|pending| &pending.kind),
        Some(PendingMutationKind::Save { overwrite: true })
    ));
    let (_, operation, _) = mutation_parts(&effects[0]);
    assert!(matches!(
        operation,
        FileOperation::Save {
            overwrite: true,
            ..
        }
    ));
}

#[test]
fn reloading_an_external_conflict_waits_for_the_load_result_before_replacing_the_buffer() {
    use carnet::app::{ConflictChoice, ExternalConflict, GlobalAction};

    let (_sandbox, mut app) = app_with_note(30, "note.md", "base");
    app.update(AppEvent::Action(AppAction::Editor(EditorCommand::Insert(
        "mine ".into(),
    ))));
    app.update(AppEvent::Action(AppAction::Global(GlobalAction::Save)));
    let (mutation_id, _, repository_root) = pending_mutation_identity(&app);
    app.update(AppEvent::MutationConflict {
        mutation_id,
        repository_id: Uuid::from_u128(30),
        repository_root,
        conflict: ExternalConflict::Modified {
            path: PathBuf::from("note.md"),
        },
    });

    let effects = app.update(AppEvent::ConflictChoice(ConflictChoice::Reload));

    assert_eq!(workspace_editor(&app).text(), "mine base");
    assert!(matches!(
        &effects[..],
        [AppEffect::LoadNote { path, .. }] if path == PathBuf::from("note.md").as_path()
    ));
    let Screen::Workspace(workspace) = &app.screen else {
        panic!("expected workspace");
    };
    fs::write(workspace.workspace.root().join("note.md"), "external").unwrap();
    let loaded = workspace
        .workspace
        .load_note(
            &workspace
                .workspace
                .resolve_note(PathBuf::from("note.md").as_path())
                .unwrap(),
        )
        .unwrap();

    let request_id = app.pending_request.as_ref().unwrap().request_id();
    app.update(AppEvent::NoteLoaded {
        request_id,
        repository_id: Uuid::from_u128(30),
        note: loaded,
    });

    assert_eq!(app.pending_request, None);
    assert_eq!(workspace_editor(&app).text(), "external");
    assert!(!workspace_editor(&app).is_dirty());
}

#[test]
fn saved_commit_failure_marks_the_buffer_clean_and_global_save_retries_only_git() {
    use carnet::{
        app::{CommitStatus, Dialog, FailureKind, GlobalAction, PendingMutationKind},
        git::{CommitOutcome, GitError},
    };

    let (_sandbox, mut app) = app_with_note(31, "note.md", "base");
    app.update(AppEvent::Action(AppAction::Editor(EditorCommand::Insert(
        "saved ".into(),
    ))));
    let save = app.update(AppEvent::Action(AppAction::Global(GlobalAction::Save)));
    let (mutation_id, repository_id, repository_root, file) =
        apply_save_effect(save.into_iter().next().unwrap());

    app.update(AppEvent::MutationSavedCommitFailed {
        mutation_id,
        repository_id,
        repository_root,
        file,
        error: GitError::CommandFailed {
            operation: "commit",
            status: Some(1),
            stderr: "hook rejected".into(),
        },
        tree: Ok(Vec::new()),
    });

    assert!(!workspace_editor(&app).is_dirty());
    assert!(matches!(
        app.status.commit,
        CommitStatus::SavedCommitFailed { .. }
    ));
    assert!(matches!(app.dialog, Some(Dialog::SavedCommitFailed { .. })));
    assert_eq!(
        app.failures.git.as_ref().map(|failure| failure.kind),
        Some(FailureKind::Git)
    );

    let retry = app.update(AppEvent::Action(AppAction::Global(GlobalAction::Save)));
    assert!(matches!(
        app.pending_mutation.as_ref().map(|pending| &pending.kind),
        Some(PendingMutationKind::RetryCommit)
    ));
    assert!(matches!(&retry[..], [AppEffect::RetryCommit { .. }]));
    assert!(
        app.update(AppEvent::Action(AppAction::Global(GlobalAction::Save)))
            .is_empty()
    );

    let (mutation_id, _, repository_root) = mutation_identity(&retry[0]);
    app.update(AppEvent::CommitRetryApplied {
        mutation_id,
        repository_id,
        repository_root,
        commit: CommitOutcome::Committed {
            revision: "abc123".into(),
        },
    });

    assert_eq!(app.pending_mutation, None);
    assert_eq!(app.saved_commit_failure, None);
    assert_eq!(app.failures.git, None);
    assert_eq!(app.dialog, None);
    assert_eq!(
        app.status.commit,
        CommitStatus::Committed {
            revision: "abc123".into(),
        }
    );
}

#[test]
fn failed_commit_retry_stays_retryable_without_rewriting_the_file() {
    use carnet::{
        app::{GlobalAction, PendingMutationKind},
        git::GitError,
    };

    let (_sandbox, mut app) = app_with_note(32, "note.md", "saved");
    enter_saved_commit_failure(&mut app, Uuid::from_u128(32));
    app.update(AppEvent::Action(AppAction::Global(GlobalAction::Save)));
    let (mutation_id, _, repository_root) = pending_mutation_identity(&app);

    app.update(AppEvent::CommitRetryFailed {
        mutation_id,
        repository_id: Uuid::from_u128(32),
        repository_root,
        error: GitError::CommandFailed {
            operation: "commit",
            status: Some(1),
            stderr: "still rejected".into(),
        },
    });

    assert_eq!(app.pending_mutation, None);
    assert!(app.saved_commit_failure.is_some());
    let retry = app.update(AppEvent::Action(AppAction::Global(GlobalAction::Save)));
    assert!(matches!(
        app.pending_mutation.as_ref().map(|pending| &pending.kind),
        Some(PendingMutationKind::RetryCommit)
    ));
    assert!(matches!(&retry[..], [AppEffect::RetryCommit { .. }]));
}

#[test]
fn git_recovery_clears_only_git_failure_and_unrelated_failure_keeps_exit_one() {
    use carnet::{
        app::{AppExitStatus, GlobalAction},
        editor::ClipboardError,
        git::CommitOutcome,
    };

    let (_sandbox, mut app) = app_with_note(81, "note.md", "base");
    app.update(AppEvent::ClipboardWritten(Err(ClipboardError::Unavailable)));
    enter_saved_commit_failure(&mut app, Uuid::from_u128(81));
    assert!(app.failures.clipboard.is_some());
    assert!(app.failures.git.is_some());

    let retry = app.update(AppEvent::Action(AppAction::Global(GlobalAction::Save)));
    let (mutation_id, _, repository_root) = mutation_identity(&retry[0]);
    app.update(AppEvent::CommitRetryApplied {
        mutation_id,
        repository_id: Uuid::from_u128(81),
        repository_root,
        commit: CommitOutcome::Committed {
            revision: "recovered".into(),
        },
    });

    assert_eq!(app.failures.git, None);
    assert!(app.failures.clipboard.is_some());
    app.update(AppEvent::Action(AppAction::Global(GlobalAction::Quit)));
    assert_eq!(app.quit.final_status, Some(AppExitStatus::Failure));
}

#[test]
fn dirty_navigation_during_commit_failure_prompts_before_saving_newer_edits() {
    use carnet::{
        app::{Dialog, DirtyChoice, NavigationAction},
        git::CommitOutcome,
    };

    let (_sandbox, mut app) = app_with_note(36, "note.md", "saved");
    enter_saved_commit_failure(&mut app, Uuid::from_u128(36));
    app.update(AppEvent::Action(AppAction::Editor(EditorCommand::Insert(
        "new ".into(),
    ))));
    app.update(AppEvent::Action(AppAction::Navigate(
        NavigationAction::Quit,
    )));

    let retry = app.update(AppEvent::DirtyChoice(DirtyChoice::Save));

    assert!(matches!(&retry[..], [AppEffect::RetryCommit { .. }]));
    assert_eq!(
        app.pending_intent,
        Some(PendingIntent::Navigation(NavigationAction::Quit))
    );

    let (mutation_id, _, repository_root) = mutation_identity(&retry[0]);
    let save = app.update(AppEvent::CommitRetryApplied {
        mutation_id,
        repository_id: Uuid::from_u128(36),
        repository_root,
        commit: CommitOutcome::Committed {
            revision: "old-save".into(),
        },
    });

    assert!(save.is_empty());
    assert_eq!(
        app.pending_intent,
        Some(PendingIntent::Navigation(NavigationAction::Quit))
    );
    assert!(matches!(app.dialog, Some(Dialog::DirtyNavigation)));
    assert!(workspace_editor(&app).is_dirty());

    let newer_save = app.update(AppEvent::DirtyChoice(DirtyChoice::Save));
    assert!(matches!(
        &newer_save[..],
        [AppEffect::ApplyAndCommit { .. }]
    ));
}

#[test]
fn failed_save_stays_on_the_dirty_note_and_marks_a_failure_exit() {
    use carnet::{
        app::{AppExitStatus, DirtyChoice, FailureKind, GlobalAction, NavigationAction},
        git::MutationCommitError,
        workspace::FileError,
    };

    let (_sandbox, mut app) = app_with_note(33, "note.md", "base");
    app.update(AppEvent::Action(AppAction::Editor(EditorCommand::Insert(
        "dirty ".into(),
    ))));
    app.update(AppEvent::Action(AppAction::Navigate(
        NavigationAction::Quit,
    )));
    app.update(AppEvent::DirtyChoice(DirtyChoice::Save));
    let (mutation_id, _, repository_root) = pending_mutation_identity(&app);

    app.update(AppEvent::MutationFailed {
        mutation_id,
        repository_id: Uuid::from_u128(33),
        repository_root,
        error: MutationCommitError::File(FileError::Io {
            path: PathBuf::from("note.md"),
            source: std::io::Error::new(std::io::ErrorKind::PermissionDenied, "read only"),
        }),
    });

    assert_eq!(app.pending_mutation, None);
    assert_eq!(
        app.pending_intent,
        Some(PendingIntent::Navigation(NavigationAction::Quit))
    );
    assert!(workspace_editor(&app).is_dirty());
    assert_eq!(
        app.failures.write.as_ref().map(|failure| failure.kind),
        Some(FailureKind::Write)
    );
    assert!(matches!(
        app.update(AppEvent::Action(AppAction::Global(GlobalAction::Save)))
            .as_slice(),
        [AppEffect::ApplyAndCommit { .. }]
    ));
    let (mutation_id, _, repository_root) = pending_mutation_identity(&app);

    app.update(AppEvent::MutationFailed {
        mutation_id,
        repository_id: Uuid::from_u128(33),
        repository_root,
        error: MutationCommitError::File(FileError::Io {
            path: PathBuf::from("note.md"),
            source: std::io::Error::new(std::io::ErrorKind::PermissionDenied, "still read only"),
        }),
    });
    app.update(AppEvent::DirtyChoice(DirtyChoice::Discard));
    assert_eq!(app.quit.final_status, Some(AppExitStatus::Failure));
}

#[test]
fn tree_refresh_failure_after_save_preserves_the_saved_result_and_reports_runtime_failure() {
    use carnet::{
        app::{FailureKind, GlobalAction},
        git::CommitOutcome,
        workspace::FileError,
    };

    let (_sandbox, mut app) = app_with_note(44, "note.md", "base");
    app.update(AppEvent::Action(AppAction::Editor(EditorCommand::Insert(
        "saved ".into(),
    ))));
    let save = app.update(AppEvent::Action(AppAction::Global(GlobalAction::Save)));
    let (mutation_id, repository_id, repository_root, file) =
        apply_save_effect(save.into_iter().next().unwrap());

    app.update(AppEvent::MutationApplied {
        mutation_id,
        repository_id,
        repository_root,
        file,
        commit: CommitOutcome::NoChanges,
        tree: Err(FileError::GitIgnore {
            message: "tree unavailable".into(),
        }),
    });

    assert!(!workspace_editor(&app).is_dirty());
    assert_eq!(app.pending_mutation, None);
    assert_eq!(
        app.failures.runtime.first().map(|_| FailureKind::Runtime),
        Some(FailureKind::Runtime),
    );
}

#[test]
fn tree_focus_routes_navigation_file_actions_and_escape_through_update() {
    use carnet::app::{Dialog, EffectExecutor, FileActionKind, Focus, TreeAction};

    let (_sandbox, mut app) = app_with_note(34, "z.md", "z");
    let (repository, workspace, git) = {
        let Screen::Workspace(workspace) = &app.screen else {
            panic!("expected workspace");
        };
        (
            workspace.repository.clone(),
            workspace.workspace.clone(),
            workspace.git.clone(),
        )
    };
    fs::create_dir(workspace.root().join("folder")).unwrap();
    fs::write(workspace.root().join("folder/child.md"), "child").unwrap();
    fs::write(workspace.root().join("a.md"), "a").unwrap();
    let tree = workspace.tree().unwrap();
    let note = workspace
        .load_note(
            &workspace
                .resolve_note(PathBuf::from("z.md").as_path())
                .unwrap(),
        )
        .unwrap();
    app.update(AppEvent::Action(AppAction::Navigate(
        carnet::app::NavigationAction::Repository {
            repository: repository.clone(),
            note: Some(PathBuf::from("z.md")),
        },
    )));
    let request_id = app.pending_request.as_ref().unwrap().request_id();
    app.update(AppEvent::WorkspaceOpened {
        request_id,
        repository_id: repository.id,
        workspace,
        git,
        tree,
        note: Some(note),
    });

    app.update(AppEvent::Action(AppAction::Focus(Focus::Tree)));
    assert_eq!(workspace_focus(&app), Focus::Tree);
    assert_eq!(workspace_tree_selection(&app), Some(0));

    app.update(AppEvent::Action(AppAction::Tree(TreeAction::Right)));
    assert!(workspace_expanded(&app).contains(PathBuf::from("folder").as_path()));
    app.update(AppEvent::Action(AppAction::Tree(TreeAction::Down)));
    assert_eq!(workspace_tree_selection(&app), Some(1));
    app.update(AppEvent::Action(AppAction::Tree(TreeAction::Up)));
    assert_eq!(workspace_tree_selection(&app), Some(0));
    app.update(AppEvent::Action(AppAction::Tree(TreeAction::Down)));
    app.update(AppEvent::Action(AppAction::Tree(TreeAction::Left)));
    assert_eq!(workspace_tree_selection(&app), Some(0));

    app.update(AppEvent::Action(AppAction::Tree(TreeAction::Right)));
    app.update(AppEvent::Action(AppAction::Tree(TreeAction::Down)));
    let opened = app.update(AppEvent::Action(AppAction::Tree(TreeAction::Open)));
    assert!(matches!(
        &opened[..],
        [AppEffect::LoadNote { path, .. }] if path == PathBuf::from("folder/child.md").as_path()
    ));
    app.update(
        EffectExecutor::default()
            .execute(opened.into_iter().next().unwrap())
            .unwrap(),
    );

    for (action, kind) in [
        (TreeAction::NewFile, FileActionKind::NewFile),
        (TreeAction::NewFolder, FileActionKind::NewFolder),
        (TreeAction::Rename, FileActionKind::Rename),
        (TreeAction::Move, FileActionKind::Move),
    ] {
        app.update(AppEvent::Action(AppAction::Tree(action)));
        assert!(matches!(
            &app.dialog,
            Some(Dialog::FileAction { kind: actual, .. }) if *actual == kind
        ));
        app.update(AppEvent::Action(AppAction::Dismiss));
    }

    app.update(AppEvent::Action(AppAction::Tree(TreeAction::Delete)));
    assert!(matches!(
        &app.dialog,
        Some(Dialog::ConfirmDelete { path, .. }) if path == PathBuf::from("folder/child.md").as_path()
    ));
    app.update(AppEvent::Action(AppAction::Dismiss));

    app.update(AppEvent::Action(AppAction::SetSidebarOverlayIntent(true)));
    app.update(AppEvent::Action(AppAction::Tree(TreeAction::Escape)));
    assert_eq!(workspace_focus(&app), Focus::Editor);
    assert!(!app.sidebar.visible);
}

#[test]
fn dirty_tree_open_uses_the_navigation_prompt_before_loading() {
    use carnet::app::{Dialog, Focus, NavigationAction, TreeAction};

    let (_sandbox, mut app) = app_with_note(74, "a.md", "a");
    let (repository, workspace, git) = {
        let Screen::Workspace(workspace) = &app.screen else {
            panic!("expected workspace");
        };
        (
            workspace.repository.clone(),
            workspace.workspace.clone(),
            workspace.git.clone(),
        )
    };
    fs::write(workspace.root().join("b.md"), "b").unwrap();
    let tree = workspace.tree().unwrap();
    let note = workspace
        .load_note(
            &workspace
                .resolve_note(PathBuf::from("a.md").as_path())
                .unwrap(),
        )
        .unwrap();
    app.update(AppEvent::Action(AppAction::Navigate(
        NavigationAction::Repository {
            repository: repository.clone(),
            note: Some(PathBuf::from("a.md")),
        },
    )));
    let request_id = app.pending_request.as_ref().unwrap().request_id();
    app.update(AppEvent::WorkspaceOpened {
        request_id,
        repository_id: repository.id,
        workspace,
        git,
        tree,
        note: Some(note),
    });
    app.update(AppEvent::Action(AppAction::Focus(Focus::Tree)));
    app.update(AppEvent::Action(AppAction::Tree(TreeAction::Down)));
    app.update(AppEvent::Action(AppAction::Editor(EditorCommand::Insert(
        "dirty ".into(),
    ))));

    let effects = app.update(AppEvent::Action(AppAction::Tree(TreeAction::Open)));

    assert!(effects.is_empty());
    assert_eq!(app.pending_request, None);
    assert_eq!(
        app.pending_intent,
        Some(PendingIntent::Navigation(NavigationAction::Note(
            PathBuf::from("b.md")
        )))
    );
    assert!(matches!(app.dialog, Some(Dialog::DirtyNavigation)));
    assert_eq!(workspace_editor(&app).text(), "dirty a");
}

#[test]
fn file_dialog_submission_emits_one_mutation_and_disables_competing_tree_mutations() {
    use carnet::{
        app::{FileActionKind, Focus, PendingMutationKind, TreeAction},
        git::CommitIntent,
        workspace::FileOperation,
    };

    let (_sandbox, mut app) = app_with_note(35, "note.md", "base");
    app.update(AppEvent::Action(AppAction::Focus(Focus::Tree)));
    app.update(AppEvent::Action(AppAction::Tree(TreeAction::NewFile)));

    let effects = app.update(AppEvent::Action(AppAction::SubmitFileAction(
        PathBuf::from("new.md"),
    )));

    assert_eq!(app.dialog, None);
    assert!(matches!(
        app.pending_mutation.as_ref().map(|pending| &pending.kind),
        Some(PendingMutationKind::File(FileActionKind::NewFile))
    ));
    let (_, operation, intent) = mutation_parts(&effects[0]);
    assert!(matches!(
        (operation, intent),
        (
            FileOperation::CreateFile { path, .. },
            CommitIntent::Create(intent_path),
        ) if path == PathBuf::from("new.md").as_path()
            && intent_path == PathBuf::from("new.md").as_path()
    ));

    app.update(AppEvent::Action(AppAction::Tree(TreeAction::NewFolder)));
    assert_eq!(app.dialog, None);
}

#[test]
fn empty_tree_allows_root_file_and_folder_creation_only() {
    use carnet::app::{Dialog, FileActionKind, Focus, TreeAction};

    let sandbox = tempdir().unwrap();
    let root = fs::canonicalize(sandbox.path()).unwrap();
    let repository = RepoEntry {
        id: Uuid::from_u128(75),
        name: "empty".into(),
        path: root.clone(),
    };
    let git = GitRepo::initialize(&root).unwrap();
    let workspace = Workspace::open(repository.clone()).unwrap();
    let mut app = App::home(vec![repository.clone()], Some(repository.id), None);
    app.update(AppEvent::Action(AppAction::Home(HomeAction::OpenSelected)));
    let request_id = app.pending_request.as_ref().unwrap().request_id();
    app.update(AppEvent::WorkspaceOpened {
        request_id,
        repository_id: repository.id,
        workspace,
        git,
        tree: Vec::new(),
        note: None,
    });
    app.update(AppEvent::Action(AppAction::Focus(Focus::Tree)));

    app.update(AppEvent::Action(AppAction::Tree(TreeAction::NewFile)));
    assert!(matches!(
        app.dialog,
        Some(Dialog::FileAction {
            kind: FileActionKind::NewFile,
            target: None,
            ..
        })
    ));
    app.update(AppEvent::Action(AppAction::Dismiss));
    app.update(AppEvent::Action(AppAction::Tree(TreeAction::NewFolder)));
    assert!(matches!(
        app.dialog,
        Some(Dialog::FileAction {
            kind: FileActionKind::NewFolder,
            target: None,
            ..
        })
    ));
    app.update(AppEvent::Action(AppAction::Dismiss));

    for action in [
        TreeAction::Open,
        TreeAction::Rename,
        TreeAction::Move,
        TreeAction::Delete,
    ] {
        assert!(
            app.update(AppEvent::Action(AppAction::Tree(action)))
                .is_empty()
        );
        assert_eq!(app.dialog, None);
    }
}

#[test]
fn folder_rename_move_and_confirmed_delete_build_their_exact_operations() {
    use carnet::{
        app::{Focus, TreeAction},
        git::CommitIntent,
        workspace::FileOperation,
    };

    let cases = [
        (TreeAction::NewFolder, PathBuf::from("folder"), "folder"),
        (TreeAction::Rename, PathBuf::from("renamed.md"), "rename"),
        (TreeAction::Move, PathBuf::from("folder/note.md"), "move"),
    ];
    for (index, (action, destination, label)) in cases.into_iter().enumerate() {
        let (_sandbox, mut app) = app_with_note(40 + index as u128, "note.md", "base");
        app.update(AppEvent::Action(AppAction::Focus(Focus::Tree)));
        app.update(AppEvent::Action(AppAction::Tree(action)));
        let effects = app.update(AppEvent::Action(AppAction::SubmitFileAction(
            destination.clone(),
        )));

        let (_, operation, intent) = mutation_parts(&effects[0]);
        match (label, operation, intent) {
            (
                "folder",
                FileOperation::CreateFolder { path, .. },
                CommitIntent::Create(intent_path),
            ) => {
                assert_eq!(path, PathBuf::from("folder").as_path());
                assert_eq!(intent_path, PathBuf::from("folder").as_path());
            }
            (
                "rename",
                FileOperation::Rename { from, to, .. },
                CommitIntent::Move {
                    from: intent_from,
                    to: intent_to,
                },
            ) => {
                assert_eq!(from, PathBuf::from("note.md").as_path());
                assert_eq!(to, PathBuf::from("renamed.md").as_path());
                assert_eq!(intent_from, from);
                assert_eq!(intent_to, to);
            }
            (
                "move",
                FileOperation::Move { from, to, .. },
                CommitIntent::Move {
                    from: intent_from,
                    to: intent_to,
                },
            ) => {
                assert_eq!(from, PathBuf::from("note.md").as_path());
                assert_eq!(to, PathBuf::from("folder/note.md").as_path());
                assert_eq!(intent_from, from);
                assert_eq!(intent_to, to);
            }
            other => panic!("unexpected {label} effect: {other:?}"),
        }
    }

    let (_sandbox, mut app) = app_with_note(43, "note.md", "base");
    app.update(AppEvent::Action(AppAction::Focus(Focus::Tree)));
    app.update(AppEvent::Action(AppAction::Tree(TreeAction::Delete)));
    let effects = app.update(AppEvent::Action(AppAction::ConfirmDelete));
    let (_, operation, intent) = mutation_parts(&effects[0]);
    assert!(matches!(
        (operation, intent),
        (
            FileOperation::Delete {
                path,
                confirmed: true,
                ..
            },
            CommitIntent::Delete(intent_path),
        ) if path == PathBuf::from("note.md").as_path()
            && intent_path == PathBuf::from("note.md").as_path()
    ));
}

#[test]
fn mutation_results_reconcile_active_note_and_tree_selection() {
    use carnet::{
        app::{Focus, TreeAction},
        git::CommitOutcome,
    };

    let (_sandbox, mut created) = empty_app(76);
    created.update(AppEvent::Action(AppAction::Focus(Focus::Tree)));
    created.update(AppEvent::Action(AppAction::Tree(TreeAction::NewFile)));
    let effect = created
        .update(AppEvent::Action(AppAction::SubmitFileAction(
            PathBuf::from("created.md"),
        )))
        .pop()
        .unwrap();
    let (mutation_id, repository_id, repository_root, file, tree) = apply_mutation_effect(effect);
    let follow_up = created.update(AppEvent::MutationApplied {
        mutation_id,
        repository_id,
        repository_root,
        file,
        commit: CommitOutcome::NoChanges,
        tree,
    });
    assert!(matches!(
        &follow_up[..],
        [AppEffect::LoadNote { path, .. }] if path == PathBuf::from("created.md").as_path()
    ));
    assert_eq!(workspace_tree_selection(&created), Some(0));

    let (_sandbox, mut folder) = empty_app(77);
    folder.update(AppEvent::Action(AppAction::Focus(Focus::Tree)));
    folder.update(AppEvent::Action(AppAction::Tree(TreeAction::NewFolder)));
    let effect = folder
        .update(AppEvent::Action(AppAction::SubmitFileAction(
            PathBuf::from("folder"),
        )))
        .pop()
        .unwrap();
    let (mutation_id, repository_id, repository_root, file, tree) = apply_mutation_effect(effect);
    assert!(
        folder
            .update(AppEvent::MutationApplied {
                mutation_id,
                repository_id,
                repository_root,
                file,
                commit: CommitOutcome::NoChanges,
                tree,
            })
            .is_empty()
    );
    assert_eq!(workspace_tree_selection(&folder), Some(0));

    for (id, action, destination) in [
        (78, TreeAction::Rename, PathBuf::from("renamed.md")),
        (79, TreeAction::Move, PathBuf::from("folder/moved.md")),
    ] {
        let (_sandbox, mut app) = app_with_note(id, "note.md", "base");
        app.update(AppEvent::Action(AppAction::Focus(Focus::Tree)));
        app.update(AppEvent::Action(AppAction::Tree(action)));
        let effect = app
            .update(AppEvent::Action(AppAction::SubmitFileAction(
                destination.clone(),
            )))
            .pop()
            .unwrap();
        let (mutation_id, repository_id, repository_root, file, tree) =
            apply_mutation_effect(effect);
        let follow_up = app.update(AppEvent::MutationApplied {
            mutation_id,
            repository_id,
            repository_root,
            file,
            commit: CommitOutcome::NoChanges,
            tree,
        });
        assert!(matches!(
            &follow_up[..],
            [AppEffect::LoadNote { path, .. }] if path == destination.as_path()
        ));
        assert_eq!(
            app.pending_request
                .as_ref()
                .and_then(|pending| pending.path()),
            Some(destination.as_path())
        );
        assert_eq!(
            selected_tree_path(&app).as_deref(),
            Some(destination.as_path())
        );
    }

    let (_sandbox, mut deleted) = app_with_note(80, "note.md", "base");
    deleted.update(AppEvent::Action(AppAction::Focus(Focus::Tree)));
    deleted.update(AppEvent::Action(AppAction::Tree(TreeAction::Delete)));
    let effect = deleted
        .update(AppEvent::Action(AppAction::ConfirmDelete))
        .pop()
        .unwrap();
    let (mutation_id, repository_id, repository_root, file, tree) = apply_mutation_effect(effect);
    assert!(
        deleted
            .update(AppEvent::MutationApplied {
                mutation_id,
                repository_id,
                repository_root,
                file,
                commit: CommitOutcome::NoChanges,
                tree,
            })
            .is_empty()
    );
    let Screen::Workspace(workspace) = &deleted.screen else {
        panic!("expected workspace");
    };
    assert_eq!(workspace.current_note, None);
    assert!(workspace.editor.is_none());
    assert_eq!(workspace.tree_selection, None);
}

#[test]
fn renaming_an_ancestor_reloads_the_active_note_at_its_rebased_path() {
    use carnet::{
        app::{Focus, TreeAction},
        git::CommitOutcome,
    };

    let (_sandbox, mut app) = app_with_note(81, "folder/sub/note.md", "base");
    app.update(AppEvent::Action(AppAction::Focus(Focus::Tree)));
    app.update(AppEvent::Action(AppAction::Tree(TreeAction::Rename)));
    let effect = app
        .update(AppEvent::Action(AppAction::SubmitFileAction(
            PathBuf::from("renamed"),
        )))
        .pop()
        .unwrap();
    let (mutation_id, repository_id, repository_root, file, tree) = apply_mutation_effect(effect);

    let follow_up = app.update(AppEvent::MutationApplied {
        mutation_id,
        repository_id,
        repository_root,
        file,
        commit: CommitOutcome::NoChanges,
        tree,
    });

    assert!(matches!(
        &follow_up[..],
        [AppEffect::LoadNote { path, .. }]
            if path == PathBuf::from("renamed/sub/note.md").as_path()
    ));
    assert_eq!(
        app.pending_request
            .as_ref()
            .and_then(|pending| pending.path()),
        Some(PathBuf::from("renamed/sub/note.md").as_path())
    );
    assert_eq!(selected_tree_path(&app), Some(PathBuf::from("renamed")));
}

#[test]
fn moving_an_ancestor_reloads_the_active_note_and_selects_the_nested_destination() {
    use carnet::{
        app::{Focus, TreeAction},
        git::CommitOutcome,
    };

    let (sandbox, mut app) = app_with_note(82, "folder/sub/note.md", "base");
    fs::create_dir(sandbox.path().join("archive")).unwrap();
    app.update(AppEvent::Action(AppAction::Focus(Focus::Tree)));
    app.update(AppEvent::Action(AppAction::Tree(TreeAction::Move)));
    let effect = app
        .update(AppEvent::Action(AppAction::SubmitFileAction(
            PathBuf::from("archive/folder"),
        )))
        .pop()
        .unwrap();
    let (mutation_id, repository_id, repository_root, file, tree) = apply_mutation_effect(effect);

    let follow_up = app.update(AppEvent::MutationApplied {
        mutation_id,
        repository_id,
        repository_root,
        file,
        commit: CommitOutcome::NoChanges,
        tree,
    });

    assert!(matches!(
        &follow_up[..],
        [AppEffect::LoadNote { path, .. }]
            if path == PathBuf::from("archive/folder/sub/note.md").as_path()
    ));
    assert_eq!(
        selected_tree_path(&app),
        Some(PathBuf::from("archive/folder"))
    );
}

#[test]
fn deleting_an_ancestor_clears_the_active_note_and_clamps_tree_selection() {
    use carnet::{
        app::{Focus, TreeAction},
        git::CommitOutcome,
    };

    let (sandbox, mut app) = app_with_note(83, "folder/sub/note.md", "base");
    fs::write(sandbox.path().join("remaining.md"), "remaining").unwrap();
    app.update(AppEvent::Action(AppAction::Focus(Focus::Tree)));
    app.update(AppEvent::Action(AppAction::Tree(TreeAction::Delete)));
    let effect = app
        .update(AppEvent::Action(AppAction::ConfirmDelete))
        .pop()
        .unwrap();
    let (mutation_id, repository_id, repository_root, file, tree) = apply_mutation_effect(effect);

    assert!(
        app.update(AppEvent::MutationApplied {
            mutation_id,
            repository_id,
            repository_root,
            file,
            commit: CommitOutcome::NoChanges,
            tree,
        })
        .is_empty()
    );
    let Screen::Workspace(workspace) = &app.screen else {
        panic!("expected workspace");
    };
    assert_eq!(workspace.current_note, None);
    assert!(workspace.editor.is_none());
    assert_eq!(
        selected_tree_path(&app),
        Some(PathBuf::from("remaining.md"))
    );
}

#[test]
fn global_editor_shortcuts_are_pure_and_clipboard_work_is_effect_driven() {
    use carnet::app::GlobalAction;

    let (_sandbox, mut app) = app_with_note(21, "note.md", "base");

    app.update(AppEvent::Action(AppAction::Editor(EditorCommand::Insert(
        "x".into(),
    ))));
    app.update(AppEvent::Action(AppAction::Global(GlobalAction::Undo)));
    assert_eq!(workspace_editor(&app).text(), "base");
    app.update(AppEvent::Action(AppAction::Global(GlobalAction::Redo)));
    assert_eq!(workspace_editor(&app).text(), "xbase");

    app.update(AppEvent::Action(AppAction::Global(GlobalAction::SelectAll)));
    let copy = app.update(AppEvent::Action(AppAction::Global(GlobalAction::Copy)));
    assert!(matches!(
        &copy[..],
        [AppEffect::WriteClipboard { text }] if text == "xbase"
    ));
    assert_eq!(workspace_editor(&app).text(), "xbase");

    let cut = app.update(AppEvent::Action(AppAction::Global(GlobalAction::Cut)));
    assert!(matches!(
        &cut[..],
        [AppEffect::WriteClipboard { text }] if text == "xbase"
    ));
    assert_eq!(workspace_editor(&app).text(), "");

    let paste = app.update(AppEvent::Action(AppAction::Global(GlobalAction::Paste)));
    assert!(matches!(&paste[..], [AppEffect::ReadClipboard]));
    assert_eq!(workspace_editor(&app).text(), "");
    app.update(AppEvent::ClipboardRead(Ok("pasted".into())));
    assert_eq!(workspace_editor(&app).text(), "pasted");
}

#[test]
fn outer_runtime_effect_failures_return_as_explicit_app_events() {
    use carnet::{app::FailureKind, editor::ClipboardError};

    let repository = repository(48, "notes");
    let mut app = App::home(vec![repository.clone()], Some(repository.id), None);

    app.update(AppEvent::ClipboardWritten(Err(ClipboardError::Unavailable)));
    assert_eq!(
        app.failures.clipboard.as_ref().map(|failure| failure.kind),
        Some(FailureKind::Runtime)
    );

    let mut catalog_app = App::home(vec![repository.clone()], Some(repository.id), None);
    catalog_app.update(AppEvent::RepositoryCatalogFailed {
        message: "notes is not registered".into(),
    });
    assert_eq!(
        catalog_app
            .failures
            .catalog
            .as_ref()
            .map(|failure| failure.kind),
        Some(FailureKind::Runtime)
    );
}

#[test]
fn global_find_quick_open_and_sidebar_actions_update_overlay_and_focus_state() {
    use carnet::app::{Focus, GlobalAction, OverlayState};

    let (_sandbox, mut app) = app_with_note(20, "note.md", "text");

    assert!(
        app.update(AppEvent::Action(AppAction::Global(GlobalAction::Find)))
            .is_empty()
    );
    assert!(matches!(app.overlay, OverlayState::Search { .. }));

    app.update(AppEvent::Action(AppAction::Global(GlobalAction::QuickOpen)));
    assert!(matches!(app.overlay, OverlayState::QuickOpen { .. }));

    assert!(app.sidebar.visible);
    app.update(AppEvent::Action(AppAction::Global(
        GlobalAction::ToggleSidebar,
    )));
    assert!(app.sidebar.visible);
    assert_eq!(workspace_focus(&app), Focus::Tree);
    app.update(AppEvent::Action(AppAction::Global(
        GlobalAction::ToggleSidebar,
    )));
    assert!(!app.sidebar.visible);
    assert_eq!(workspace_focus(&app), Focus::Editor);
    app.update(AppEvent::Action(AppAction::SetSidebarOverlayIntent(true)));
    assert!(app.sidebar.overlay_intent);
}

#[test]
fn choosing_a_default_preserves_the_pending_note_until_workspace_open_finishes() {
    let repository = repository(7, "chosen");
    let pending_note = PathBuf::from("inbox/today.md");
    let mut app = App::home(vec![repository.clone()], None, Some(pending_note.clone()));

    assert_eq!(
        app.home.default_choice,
        DefaultChoiceState::AwaitingSelection
    );

    let effects = app.update(AppEvent::Action(AppAction::Home(
        HomeAction::ChooseSelectedAsDefault,
    )));

    assert_eq!(app.home.default_repository, None);
    assert_eq!(
        app.home.default_choice,
        DefaultChoiceState::AwaitingSelection
    );
    assert_eq!(app.home.pending_note, Some(pending_note.clone()));
    assert!(matches!(
        &effects[..],
        [AppEffect::SetDefaultRepository { repository_id }]
            if *repository_id == repository.id
    ));
    assert!(app.pending_request.is_none());

    let effects = app.update(AppEvent::RepositoryCatalogChanged(CatalogSnapshot {
        repositories: vec![repository.clone()],
        default_repository: Some(repository.id),
        selected_repository: Some(repository.id),
    }));

    assert_eq!(app.home.default_repository, Some(repository.id));
    assert_eq!(
        app.home.default_choice,
        DefaultChoiceState::ResumingPendingNote {
            repository_id: repository.id,
            note: pending_note.clone(),
        }
    );
    assert!(matches!(
        &effects[..],
        [AppEffect::OpenWorkspace {
            repository: opened,
            note: Some(note),
            ..
        }] if opened == &repository && note == &pending_note
    ));
}

#[test]
fn enter_with_a_pending_note_persists_the_default_before_opening() {
    let repository = repository(70, "chosen");
    let pending_note = PathBuf::from("inbox/today.md");
    let mut app = App::home(vec![repository.clone()], None, Some(pending_note.clone()));

    let effects = app.update(AppEvent::Action(AppAction::Home(HomeAction::OpenSelected)));

    assert!(matches!(
        effects.as_slice(),
        [AppEffect::SetDefaultRepository { repository_id }]
            if *repository_id == repository.id
    ));
    assert_eq!(app.home.default_repository, None);
    assert_eq!(app.home.pending_note, Some(pending_note));
    assert!(app.pending_request.is_none());
}

#[test]
fn failed_default_persistence_keeps_the_previous_default_and_pending_note() {
    let first = repository(71, "first");
    let chosen = repository(72, "chosen");
    let note = PathBuf::from("pending.md");
    let mut app = App::home(
        vec![first.clone(), chosen.clone()],
        Some(first.id),
        Some(note.clone()),
    );
    app.home.selected = Some(1);

    app.update(AppEvent::Action(AppAction::Home(
        HomeAction::ChooseSelectedAsDefault,
    )));
    let effects = app.update(AppEvent::RepositoryCatalogFailed {
        message: "catalog save failed".into(),
    });

    assert!(effects.is_empty());
    assert_eq!(app.home.default_repository, Some(first.id));
    assert_eq!(app.home.pending_note, Some(note));
    assert!(app.pending_request.is_none());
    assert!(app.failures.catalog.is_some());
}

#[test]
fn opening_the_existing_default_resumes_its_pending_note() {
    let repository = repository(6, "default");
    let note = PathBuf::from("today.md");
    let mut app = App::home(
        vec![repository.clone()],
        Some(repository.id),
        Some(note.clone()),
    );

    let effects = app.update(AppEvent::Action(AppAction::Home(HomeAction::OpenSelected)));

    assert_eq!(
        app.home.default_choice,
        DefaultChoiceState::ResumingPendingNote {
            repository_id: repository.id,
            note: note.clone(),
        }
    );
    assert!(matches!(
        &effects[..],
        [AppEffect::OpenWorkspace {
            repository: opened,
            note: Some(opened_note),
            ..
        }] if opened == &repository && opened_note == &note
    ));
}

#[test]
fn workspace_open_result_resumes_the_pending_note_and_clears_home_resume_state() {
    let (sandbox, repository, workspace, git) = workspace_fixture(8, "notes", "draft.md", "hello");
    let tree = workspace.tree().unwrap();
    let note = workspace
        .load_note(
            &workspace
                .resolve_note(PathBuf::from("draft.md").as_path())
                .unwrap(),
        )
        .unwrap();
    let mut app = App::home(
        vec![repository.clone()],
        None,
        Some(PathBuf::from("draft.md")),
    );
    app.update(AppEvent::Action(AppAction::Home(
        HomeAction::ChooseSelectedAsDefault,
    )));

    let open = app.update(AppEvent::RepositoryCatalogChanged(CatalogSnapshot {
        repositories: vec![repository.clone()],
        default_repository: Some(repository.id),
        selected_repository: Some(repository.id),
    }));
    assert!(matches!(open.as_slice(), [AppEffect::OpenWorkspace { .. }]));

    let request_id = app.pending_request.as_ref().unwrap().request_id();
    let effects = app.update(AppEvent::WorkspaceOpened {
        request_id,
        repository_id: repository.id,
        workspace,
        git,
        tree,
        note: Some(note),
    });

    assert!(effects.is_empty());
    assert_eq!(app.home.pending_note, None);
    assert_eq!(app.home.default_choice, DefaultChoiceState::NotNeeded);
    let Screen::Workspace(workspace) = &app.screen else {
        panic!("expected workspace screen");
    };
    assert_eq!(workspace.repository, repository);
    assert_eq!(
        workspace.current_note.as_deref(),
        Some(PathBuf::from("draft.md").as_path())
    );
    assert_eq!(workspace.editor.as_ref().unwrap().text(), "hello");
    drop(sandbox);
}

#[test]
fn catalog_rename_or_unregister_invalidates_a_pending_workspace_open() {
    for repositories in [
        Vec::new(),
        vec![RepoEntry {
            id: Uuid::from_u128(73),
            name: "renamed".into(),
            path: PathBuf::from("/repos/original"),
        }],
    ] {
        let (sandbox, repository, workspace, git) =
            workspace_fixture(73, "original", "note.md", "hello");
        let tree = workspace.tree().unwrap();
        let note = workspace
            .load_note(
                &workspace
                    .resolve_note(PathBuf::from("note.md").as_path())
                    .unwrap(),
            )
            .unwrap();
        let mut app = App::home(vec![repository.clone()], Some(repository.id), None);
        app.update(AppEvent::Action(AppAction::Home(HomeAction::OpenSelected)));
        let request_id = app.pending_request.as_ref().unwrap().request_id();

        app.update(AppEvent::RepositoryCatalogChanged(CatalogSnapshot {
            repositories,
            default_repository: None,
            selected_repository: None,
        }));
        assert!(app.pending_request.is_none());

        app.update(AppEvent::WorkspaceOpened {
            request_id,
            repository_id: repository.id,
            workspace,
            git,
            tree,
            note: Some(note),
        });

        assert!(matches!(app.screen, Screen::Home));
        drop(sandbox);
    }
}

#[test]
fn pending_workspace_open_suppresses_repository_home_mutations() {
    let repository = repository(74, "notes");

    for action in [
        HomeAction::CreateRepository,
        HomeAction::RegisterRepository,
        HomeAction::RenameSelected,
        HomeAction::SetDefaultSelected,
        HomeAction::UnregisterSelected,
    ] {
        let mut app = App::home(vec![repository.clone()], Some(repository.id), None);
        app.update(AppEvent::Action(AppAction::Home(HomeAction::OpenSelected)));

        assert!(
            app.update(AppEvent::Action(AppAction::Home(action)))
                .is_empty()
        );
        assert!(app.dialog.is_none());
        assert!(app.pending_catalog.is_none());
    }
}

fn repository(id: u128, name: &str) -> RepoEntry {
    RepoEntry {
        id: Uuid::from_u128(id),
        name: name.into(),
        path: PathBuf::from(format!("/repos/{name}")),
    }
}

fn workspace_fixture(
    id: u128,
    name: &str,
    note_path: &str,
    contents: &str,
) -> (TempDir, RepoEntry, Workspace, GitRepo) {
    let sandbox = tempdir().unwrap();
    let root = fs::canonicalize(sandbox.path()).unwrap();
    let repository = RepoEntry {
        id: Uuid::from_u128(id),
        name: name.into(),
        path: root.clone(),
    };
    let git = GitRepo::initialize(&root).unwrap();
    let note_path = root.join(note_path);
    if let Some(parent) = note_path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(note_path, contents).unwrap();
    let workspace = Workspace::open(repository.clone()).unwrap();
    (sandbox, repository, workspace, git)
}

fn app_with_note(id: u128, note_path: &str, contents: &str) -> (TempDir, App) {
    let (sandbox, repository, workspace, git) = workspace_fixture(id, "notes", note_path, contents);
    let tree = workspace.tree().unwrap();
    let note = workspace
        .load_note(
            &workspace
                .resolve_note(PathBuf::from(note_path).as_path())
                .unwrap(),
        )
        .unwrap();
    let mut app = App::home(vec![repository.clone()], Some(repository.id), None);
    app.update(AppEvent::Action(AppAction::Home(HomeAction::OpenSelected)));
    let request_id = app.pending_request.as_ref().unwrap().request_id();
    app.update(AppEvent::WorkspaceOpened {
        request_id,
        repository_id: repository.id,
        workspace,
        git,
        tree,
        note: Some(note),
    });
    (sandbox, app)
}

fn empty_app(id: u128) -> (TempDir, App) {
    let sandbox = tempdir().unwrap();
    let root = fs::canonicalize(sandbox.path()).unwrap();
    let repository = RepoEntry {
        id: Uuid::from_u128(id),
        name: "empty".into(),
        path: root.clone(),
    };
    let git = GitRepo::initialize(&root).unwrap();
    let workspace = Workspace::open(repository.clone()).unwrap();
    let mut app = App::home(vec![repository.clone()], Some(repository.id), None);
    app.update(AppEvent::Action(AppAction::Home(HomeAction::OpenSelected)));
    let request_id = app.pending_request.as_ref().unwrap().request_id();
    app.update(AppEvent::WorkspaceOpened {
        request_id,
        repository_id: repository.id,
        workspace,
        git,
        tree: Vec::new(),
        note: None,
    });
    (sandbox, app)
}

fn workspace_editor(app: &App) -> &carnet::editor::Editor {
    let Screen::Workspace(workspace) = &app.screen else {
        panic!("expected workspace screen");
    };
    workspace.editor.as_ref().expect("open editor")
}

fn workspace_focus(app: &App) -> carnet::app::Focus {
    let Screen::Workspace(workspace) = &app.screen else {
        panic!("expected workspace screen");
    };
    workspace.focus
}

fn workspace_tree_selection(app: &App) -> Option<usize> {
    let Screen::Workspace(workspace) = &app.screen else {
        panic!("expected workspace screen");
    };
    workspace.tree_selection
}

fn workspace_expanded(app: &App) -> &std::collections::BTreeSet<PathBuf> {
    let Screen::Workspace(workspace) = &app.screen else {
        panic!("expected workspace screen");
    };
    &workspace.expanded
}

fn selected_tree_path(app: &App) -> Option<PathBuf> {
    fn collect(
        output: &mut Vec<PathBuf>,
        entries: &[carnet::workspace::TreeEntry],
        expanded: &std::collections::BTreeSet<PathBuf>,
    ) {
        for entry in entries {
            output.push(entry.path().to_path_buf());
            if entry.kind() == carnet::workspace::TreeEntryKind::Directory
                && expanded.contains(entry.path())
            {
                collect(output, entry.children(), expanded);
            }
        }
    }

    let Screen::Workspace(workspace) = &app.screen else {
        return None;
    };
    let mut paths = Vec::new();
    collect(&mut paths, &workspace.tree, &workspace.expanded);
    workspace
        .tree_selection
        .and_then(|selected| paths.get(selected).cloned())
}

fn mutation_parts(
    effect: &AppEffect,
) -> (
    Uuid,
    &carnet::workspace::FileOperation,
    &carnet::git::CommitIntent,
) {
    let AppEffect::ApplyAndCommit {
        repository_id,
        operation,
        intent,
        ..
    } = effect
    else {
        panic!("expected mutation effect, got {effect:?}");
    };
    (*repository_id, operation.as_ref(), intent)
}

fn mutation_identity(effect: &AppEffect) -> (carnet::app::MutationId, Uuid, PathBuf) {
    let (AppEffect::ApplyAndCommit {
        mutation_id,
        repository_id,
        repository_root,
        ..
    }
    | AppEffect::RetryCommit {
        mutation_id,
        repository_id,
        repository_root,
        ..
    }) = effect
    else {
        panic!("expected mutation effect, got {effect:?}");
    };
    (*mutation_id, *repository_id, repository_root.clone())
}

fn pending_mutation_identity(app: &App) -> (carnet::app::MutationId, Uuid, PathBuf) {
    let pending = app
        .pending_mutation
        .as_ref()
        .expect("expected pending mutation");
    (
        pending.mutation_id,
        pending.repository_id,
        pending.repository_root.clone(),
    )
}

fn apply_save_effect(
    effect: AppEffect,
) -> (
    carnet::app::MutationId,
    Uuid,
    PathBuf,
    carnet::workspace::FileOutcome,
) {
    let AppEffect::ApplyAndCommit {
        mutation_id,
        repository_id,
        repository_root,
        operation,
        ..
    } = effect
    else {
        panic!("expected mutation effect, got {effect:?}");
    };
    assert!(matches!(
        operation.as_ref(),
        carnet::workspace::FileOperation::Save { .. }
    ));
    (
        mutation_id,
        repository_id,
        repository_root,
        Workspace::apply(*operation).expect("save operation should apply"),
    )
}

fn apply_mutation_effect(
    effect: AppEffect,
) -> (
    carnet::app::MutationId,
    Uuid,
    PathBuf,
    carnet::workspace::FileOutcome,
    Result<Vec<carnet::workspace::TreeEntry>, carnet::workspace::FileError>,
) {
    let AppEffect::ApplyAndCommit {
        mutation_id,
        repository_id,
        repository_root,
        workspace,
        operation,
        ..
    } = effect
    else {
        panic!("expected mutation effect, got {effect:?}");
    };
    let file = Workspace::apply(*operation).expect("mutation should apply");
    let tree = workspace.tree();
    (mutation_id, repository_id, repository_root, file, tree)
}

fn enter_saved_commit_failure(app: &mut App, repository_id: Uuid) {
    app.update(AppEvent::Action(AppAction::Editor(EditorCommand::Insert(
        "persisted ".into(),
    ))));
    let effects = app.update(AppEvent::Action(AppAction::Global(
        carnet::app::GlobalAction::Save,
    )));
    let (mutation_id, effect_repository_id, repository_root, file) =
        apply_save_effect(effects.into_iter().next().unwrap());
    assert_eq!(effect_repository_id, repository_id);
    app.update(AppEvent::MutationSavedCommitFailed {
        mutation_id,
        repository_id,
        repository_root,
        file,
        error: carnet::git::GitError::CommandFailed {
            operation: "commit",
            status: Some(1),
            stderr: "commit failed".into(),
        },
        tree: Ok(Vec::new()),
    });
}
