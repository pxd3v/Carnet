use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap},
};

use crate::app::{
    App, Dialog, ExternalConflict, FailureKind, FileActionKind, OverlayState, Screen,
};

pub(super) fn render(frame: &mut Frame<'_>, app: &App) {
    if let Some(dialog) = &app.dialog {
        render_dialog(frame, app, dialog);
        return;
    }
    match &app.overlay {
        OverlayState::None => {}
        OverlayState::Search { query } => render_search(frame, query),
        OverlayState::QuickOpen { query, selected } => {
            render_quick_open(frame, app, query, *selected);
        }
    }
}

fn render_dialog(frame: &mut Frame<'_>, app: &App, dialog: &Dialog) {
    let (title, lines, height, accent) = match dialog {
        Dialog::DirtyNavigation => (
            " Unsaved changes ",
            vec![
                Line::from("This note has changes that have not been saved."),
                Line::from(""),
                choice_line(&[("s", "Save"), ("d", "Discard"), ("Esc", "Cancel")]),
            ],
            7,
            Color::Yellow,
        ),
        Dialog::ExternalConflict(conflict) => {
            let (verb, path) = match conflict {
                ExternalConflict::Modified { path } => ("modified", path),
                ExternalConflict::Deleted { path } => ("deleted", path),
            };
            (
                " External conflict ",
                vec![
                    Line::from(format!("{} was {verb} outside Carnet.", path.display())),
                    Line::from(
                        "Reload uses the disk version; overwrite keeps this editor version.",
                    ),
                    Line::from(""),
                    choice_line(&[("r", "Reload"), ("o", "Overwrite"), ("Esc", "Cancel")]),
                ],
                8,
                Color::Yellow,
            )
        }
        Dialog::SavedCommitFailed { message } => (
            " Git failure ",
            vec![
                Line::from(Span::styled(
                    "The file was saved, but it was not committed.",
                    Style::default().add_modifier(Modifier::BOLD),
                )),
                Line::from(message.clone()),
                Line::from(""),
                choice_line(&[("r", "Retry commit"), ("Esc", "Continue editing")]),
            ],
            8,
            Color::Red,
        ),
        Dialog::Failure { kind, message } => {
            let title = match kind {
                FailureKind::Runtime => " Runtime failure ",
                FailureKind::Write => " Write failure ",
                FailureKind::Git => " Git failure ",
            };
            (
                title,
                vec![
                    Line::from(message.clone()),
                    Line::from(""),
                    choice_line(&[("Enter/Esc", "Dismiss")]),
                ],
                7,
                Color::Red,
            )
        }
        Dialog::FileAction { kind, target, .. } => {
            let (title, prompt) = match kind {
                FileActionKind::NewFile => (" New file ", "Path for the new file"),
                FileActionKind::NewFolder => (" New folder ", "Path for the new folder"),
                FileActionKind::Rename => (" Rename ", "New path"),
                FileActionKind::Move => (" Move ", "Destination path"),
            };
            let context = target
                .as_ref()
                .map(|path| format!("Selected: {}", path.display()))
                .unwrap_or_else(|| "At repository root".into());
            (
                title,
                vec![
                    Line::from(context),
                    Line::from(format!("{prompt}: {}_", app.dialog_input)),
                    Line::from(""),
                    choice_line(&[("Enter", "Submit"), ("Esc", "Cancel")]),
                ],
                8,
                Color::Cyan,
            )
        }
        Dialog::ConfirmDelete { path, .. } => (
            " Confirm delete ",
            vec![
                Line::from(format!("Delete {}?", path.display())),
                Line::from("This removes the selected file or folder from disk."),
                Line::from(""),
                choice_line(&[("y/Enter", "Delete"), ("n/Esc", "Cancel")]),
            ],
            8,
            Color::Red,
        ),
    };
    let area = centered(frame.area(), 66, height);
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(Text::from(lines))
            .wrap(Wrap { trim: false })
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(accent))
                    .title(title),
            ),
        area,
    );
}

fn render_search(frame: &mut Frame<'_>, query: &str) {
    let area = centered(frame.area(), 64, 5);
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(format!("Find: {query}_")),
            choice_line(&[
                ("Enter", "Next"),
                ("Shift+Enter", "Previous"),
                ("Esc", "Close"),
            ]),
        ])
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan))
                .title(" Find in note "),
        ),
        area,
    );
}

fn render_quick_open(frame: &mut Frame<'_>, app: &App, query: &str, selected: Option<usize>) {
    let files = match &app.screen {
        Screen::Workspace(workspace) => workspace.matching_text_paths(query),
        Screen::Home => Vec::new(),
    };
    let height = (files.len() as u16 + 5).clamp(7, 16);
    let area = centered(frame.area(), 68, height);
    frame.render_widget(Clear, area);
    let mut items = vec![ListItem::new(Line::from(format!("Open: {query}_")))];
    if files.is_empty() {
        items.push(ListItem::new(Line::styled(
            "No matching text files",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        items.extend(files.into_iter().enumerate().map(|(index, path)| {
            let style = if selected.unwrap_or(0) == index {
                Style::default().fg(Color::Black).bg(Color::Cyan)
            } else {
                Style::default()
            };
            ListItem::new(Line::styled(format!("{}", path.display()), style))
        }));
    }
    items.push(ListItem::new(Line::default()));
    items.push(ListItem::new(choice_line(&[
        ("↑/↓", "Select"),
        ("Enter", "Open"),
        ("Esc", "Close"),
    ])));
    frame.render_widget(
        List::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan))
                .title(" Quick open "),
        ),
        area,
    );
}

fn choice_line(choices: &[(&str, &str)]) -> Line<'static> {
    let mut spans = Vec::new();
    for (index, (key, label)) in choices.iter().enumerate() {
        if index > 0 {
            spans.push(Span::raw("   "));
        }
        spans.push(Span::styled(
            format!("[{key}]"),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::raw(format!(" {label}")));
    }
    Line::from(spans)
}

fn centered(area: Rect, desired_width: u16, desired_height: u16) -> Rect {
    let width = desired_width.min(area.width.saturating_sub(2)).max(1);
    let height = desired_height.min(area.height.saturating_sub(2)).max(1);
    Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    }
}
