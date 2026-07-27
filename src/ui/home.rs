use ratatui::{
    Frame,
    layout::{Constraint, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Paragraph},
};

use crate::app::{App, DefaultChoiceState, RepositoryAvailability};

pub(super) fn render(frame: &mut Frame<'_>, app: &App) {
    let [header, body, help] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(5),
        Constraint::Length(4),
    ])
    .areas(frame.area());

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                " Carnet ",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("Repository home"),
        ]))
        .block(Block::default().borders(Borders::ALL)),
        header,
    );

    let content = if app.home.repositories.is_empty() {
        let mut lines = vec![
            Line::from(""),
            Line::from(Span::styled(
                "  No repositories registered yet.",
                Style::default().add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from("  Create a repository with [c] or register an existing one with [a]."),
            Line::from("  Carnet will mark the first repository as your default."),
        ];
        if let Some(note) = &app.home.pending_note {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                format!("  Choose a repository to resume: {}", note.display()),
                Style::default().fg(Color::Yellow),
            )));
        }
        Text::from(lines)
    } else {
        let mut lines = vec![Line::from(Span::styled(
            "  Registered repositories",
            Style::default().add_modifier(Modifier::BOLD),
        ))];
        for (index, repository) in app.home.repositories.iter().enumerate() {
            let selected = app.home.selected == Some(index);
            let default = app.home.default_repository == Some(repository.id);
            let available = app
                .home
                .repository_availability
                .get(index)
                .is_some_and(|availability| *availability == RepositoryAvailability::Available);
            let marker = if selected { ">" } else { " " };
            let default_marker = if default { "default" } else { "       " };
            let status = if available {
                "ready"
            } else {
                "missing · disabled"
            };
            let style = if selected {
                Style::default().fg(Color::Black).bg(Color::Cyan)
            } else if available {
                Style::default()
            } else {
                Style::default().fg(Color::DarkGray)
            };
            lines.push(Line::styled(
                format!(
                    " {marker} {:<18}  {:<7}  {:<18}  {}",
                    repository.name,
                    default_marker,
                    status,
                    repository.path.display()
                ),
                style,
            ));
        }
        lines.push(Line::from(""));
        if app.home.default_repository.is_none() {
            lines.push(Line::from(Span::styled(
                "  No default repository. Select one and press [d].",
                Style::default().fg(Color::Yellow),
            )));
        }
        if let Some(note) = &app.home.pending_note {
            let guidance = match app.home.default_choice {
                DefaultChoiceState::AwaitingSelection => "Choose a repository to resume",
                DefaultChoiceState::ResumingPendingNote { .. } => "Opening pending note",
                DefaultChoiceState::NotNeeded => "Pending note",
            };
            lines.push(Line::from(format!("  {guidance}: {}", note.display())));
        }
        Text::from(lines)
    };
    frame.render_widget(
        Paragraph::new(content).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Repositories "),
        ),
        body,
    );

    frame.render_widget(
        Paragraph::new(vec![
            Line::from(" [c] Create  [a] Register  [Enter] Open  [r] Rename  [d] Default"),
            Line::from(" [u] Unregister  [Ctrl+Q] Quit"),
        ])
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Repository actions "),
        ),
        help,
    );
}
