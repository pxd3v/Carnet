use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::{
    app::{
        App, AppAction, AppEvent, ConflictChoice, Dialog, DirtyChoice, Focus, GlobalAction,
        HomeAction, OverlayState, Screen, TreeAction,
    },
    editor::{EditorCommand, Motion},
};

pub fn map_key(app: &App, key: KeyEvent) -> Option<AppEvent> {
    if key.kind == KeyEventKind::Release {
        return None;
    }
    if let Some(action) = global_action(key) {
        return Some(AppEvent::Action(AppAction::Global(action)));
    }
    if let Some(event) = dialog_event(app, key) {
        return Some(event);
    }
    if let Some(action) = overlay_action(app, key) {
        return Some(AppEvent::Action(action));
    }
    match &app.screen {
        Screen::Home => home_action(key).map(|action| AppEvent::Action(AppAction::Home(action))),
        Screen::Workspace(workspace) if workspace.focus == Focus::Tree => {
            tree_action(key).map(|action| AppEvent::Action(AppAction::Tree(action)))
        }
        Screen::Workspace(_) => {
            editor_action(key).map(|action| AppEvent::Action(AppAction::Editor(action)))
        }
    }
}

fn global_action(key: KeyEvent) -> Option<GlobalAction> {
    if !key.modifiers.contains(KeyModifiers::CONTROL) {
        return None;
    }
    let KeyCode::Char(character) = key.code else {
        return None;
    };
    let character = character.to_ascii_lowercase();
    match character {
        's' => Some(GlobalAction::Save),
        'f' => Some(GlobalAction::Find),
        'p' => Some(GlobalAction::QuickOpen),
        'b' => Some(GlobalAction::ToggleSidebar),
        'z' if key.modifiers.contains(KeyModifiers::SHIFT) => Some(GlobalAction::Redo),
        'z' => Some(GlobalAction::Undo),
        'y' => Some(GlobalAction::Redo),
        'c' => Some(GlobalAction::Copy),
        'x' => Some(GlobalAction::Cut),
        'v' => Some(GlobalAction::Paste),
        'a' => Some(GlobalAction::SelectAll),
        'q' => Some(GlobalAction::Quit),
        _ => None,
    }
}

fn dialog_event(app: &App, key: KeyEvent) -> Option<AppEvent> {
    let dialog = app.dialog.as_ref()?;
    match dialog {
        Dialog::DirtyNavigation => match plain_character(key) {
            Some('s') => Some(AppEvent::DirtyChoice(DirtyChoice::Save)),
            Some('d') => Some(AppEvent::DirtyChoice(DirtyChoice::Discard)),
            Some('c') => Some(AppEvent::DirtyChoice(DirtyChoice::Cancel)),
            _ if key.code == KeyCode::Esc => Some(AppEvent::DirtyChoice(DirtyChoice::Cancel)),
            _ => None,
        },
        Dialog::ExternalConflict(_) => match plain_character(key) {
            Some('r') => Some(AppEvent::ConflictChoice(ConflictChoice::Reload)),
            Some('o') => Some(AppEvent::ConflictChoice(ConflictChoice::Overwrite)),
            Some('c') => Some(AppEvent::ConflictChoice(ConflictChoice::Cancel)),
            _ if key.code == KeyCode::Esc => Some(AppEvent::ConflictChoice(ConflictChoice::Cancel)),
            _ => None,
        },
        Dialog::SavedCommitFailed { .. } => match plain_character(key) {
            Some('r') | Some('s') => Some(AppEvent::Action(AppAction::Global(GlobalAction::Save))),
            _ if key.code == KeyCode::Esc => Some(AppEvent::Action(AppAction::Dismiss)),
            _ => None,
        },
        Dialog::ConfirmDelete { .. } => match plain_character(key) {
            Some('y') if key.code == KeyCode::Char('y') => {
                Some(AppEvent::Action(AppAction::ConfirmDelete))
            }
            Some('n') => Some(AppEvent::Action(AppAction::Dismiss)),
            _ if key.code == KeyCode::Enter => Some(AppEvent::Action(AppAction::ConfirmDelete)),
            _ if key.code == KeyCode::Esc => Some(AppEvent::Action(AppAction::Dismiss)),
            _ => None,
        },
        Dialog::FileAction { .. } => match key.code {
            KeyCode::Esc => Some(AppEvent::Action(AppAction::Dismiss)),
            KeyCode::Enter if !app.dialog_input.is_empty() => Some(AppEvent::Action(
                AppAction::SubmitFileAction(app.dialog_input.clone().into()),
            )),
            KeyCode::Backspace => {
                let mut input = app.dialog_input.clone();
                input.pop();
                Some(AppEvent::Action(AppAction::SetDialogInput(input)))
            }
            KeyCode::Char(character)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                let mut input = app.dialog_input.clone();
                input.push(character);
                Some(AppEvent::Action(AppAction::SetDialogInput(input)))
            }
            _ => None,
        },
        Dialog::Failure { .. } => (key.code == KeyCode::Esc || key.code == KeyCode::Enter)
            .then_some(AppEvent::Action(AppAction::Dismiss)),
    }
}

fn overlay_action(app: &App, key: KeyEvent) -> Option<AppAction> {
    let query = match &app.overlay {
        OverlayState::Search { query } | OverlayState::QuickOpen { query, .. } => query,
        OverlayState::None => return None,
    };
    match key.code {
        KeyCode::Esc => Some(AppAction::Dismiss),
        KeyCode::Backspace => {
            let mut query = query.clone();
            query.pop();
            Some(AppAction::SetOverlayQuery(query))
        }
        KeyCode::Enter if matches!(app.overlay, OverlayState::Search { .. }) => {
            let command = if key.modifiers.contains(KeyModifiers::SHIFT) {
                EditorCommand::FindPrevious
            } else {
                EditorCommand::FindNext
            };
            Some(AppAction::Editor(command))
        }
        KeyCode::Enter if matches!(app.overlay, OverlayState::QuickOpen { .. }) => {
            Some(AppAction::SubmitOverlay)
        }
        KeyCode::Up if matches!(app.overlay, OverlayState::QuickOpen { .. }) => {
            Some(AppAction::MoveOverlaySelection(-1))
        }
        KeyCode::Down if matches!(app.overlay, OverlayState::QuickOpen { .. }) => {
            Some(AppAction::MoveOverlaySelection(1))
        }
        KeyCode::Char(character)
            if !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
        {
            let mut query = query.clone();
            query.push(character);
            Some(AppAction::SetOverlayQuery(query))
        }
        _ => None,
    }
}

fn home_action(key: KeyEvent) -> Option<HomeAction> {
    match key.code {
        KeyCode::Up => Some(HomeAction::Up),
        KeyCode::Down => Some(HomeAction::Down),
        KeyCode::Enter => Some(HomeAction::OpenSelected),
        KeyCode::Char('d') if key.modifiers == KeyModifiers::NONE => {
            Some(HomeAction::ChooseSelectedAsDefault)
        }
        _ => None,
    }
}

fn tree_action(key: KeyEvent) -> Option<TreeAction> {
    match key.code {
        KeyCode::Up => Some(TreeAction::Up),
        KeyCode::Down => Some(TreeAction::Down),
        KeyCode::Left => Some(TreeAction::Left),
        KeyCode::Right => Some(TreeAction::Right),
        KeyCode::Enter => Some(TreeAction::Open),
        KeyCode::Char('n') if !key.modifiers.contains(KeyModifiers::SHIFT) => {
            Some(TreeAction::NewFile)
        }
        KeyCode::Char('N') | KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::SHIFT) => {
            Some(TreeAction::NewFolder)
        }
        KeyCode::Char('r') if key.modifiers == KeyModifiers::NONE => Some(TreeAction::Rename),
        KeyCode::Char('m') if key.modifiers == KeyModifiers::NONE => Some(TreeAction::Move),
        KeyCode::Delete => Some(TreeAction::Delete),
        KeyCode::Esc => Some(TreeAction::Escape),
        _ => None,
    }
}

fn editor_action(key: KeyEvent) -> Option<EditorCommand> {
    let extend_selection = key.modifiers.contains(KeyModifiers::SHIFT);
    let motion = match key.code {
        KeyCode::Left => Some(Motion::Left),
        KeyCode::Right => Some(Motion::Right),
        KeyCode::Up => Some(Motion::Up),
        KeyCode::Down => Some(Motion::Down),
        KeyCode::Home => Some(Motion::LineStart),
        KeyCode::End => Some(Motion::LineEnd),
        _ => None,
    };
    if let Some(motion) = motion {
        return Some(EditorCommand::Move {
            motion,
            extend_selection,
        });
    }
    match key.code {
        KeyCode::Enter => Some(EditorCommand::Newline),
        KeyCode::Backspace => Some(EditorCommand::Backspace),
        KeyCode::Delete => Some(EditorCommand::Delete),
        KeyCode::Tab => Some(EditorCommand::Indent),
        KeyCode::BackTab => Some(EditorCommand::Outdent),
        KeyCode::Char(character)
            if !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
        {
            Some(EditorCommand::Insert(character.to_string()))
        }
        _ => None,
    }
}

fn plain_character(key: KeyEvent) -> Option<char> {
    if key
        .modifiers
        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
    {
        return None;
    }
    let KeyCode::Char(character) = key.code else {
        return None;
    };
    Some(character.to_ascii_lowercase())
}
