use std::path::PathBuf;

use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph},
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use super::{COMFORTABLE_WIDTH, ShortcutStyle, selection_viewport};
use crate::{
    app::{App, CommitStatus, Focus, PendingMutationKind, PushStatus, WorkspaceState},
    editor::{Editor, HighlightLanguage, HighlightSpan, HighlightStyle},
    workspace::{TreeEntry, TreeEntryKind},
};

const TREE_WIDTH: u16 = 30;
const NARROW_TREE_WIDTH: u16 = 34;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WorkspaceGeometry {
    pub editor: Rect,
    pub tree: Option<Rect>,
    pub tree_is_overlay: bool,
}

pub fn workspace_geometry(area: Rect, sidebar_visible: bool) -> WorkspaceGeometry {
    if !sidebar_visible {
        return WorkspaceGeometry {
            editor: area,
            tree: None,
            tree_is_overlay: false,
        };
    }
    if area.width < COMFORTABLE_WIDTH {
        return WorkspaceGeometry {
            editor: area,
            tree: Some(Rect {
                x: area.x,
                y: area.y,
                width: NARROW_TREE_WIDTH.min(area.width.saturating_sub(2)).max(1),
                height: area.height,
            }),
            tree_is_overlay: true,
        };
    }
    let [tree, editor] =
        Layout::horizontal([Constraint::Length(TREE_WIDTH), Constraint::Min(20)]).areas(area);
    WorkspaceGeometry {
        editor,
        tree: Some(tree),
        tree_is_overlay: false,
    }
}

pub(super) fn render(
    frame: &mut Frame<'_>,
    app: &App,
    workspace: &WorkspaceState,
    shortcut_style: ShortcutStyle,
) {
    let [main, shortcuts, status] = Layout::vertical([
        Constraint::Min(5),
        Constraint::Length(3),
        Constraint::Length(1),
    ])
    .areas(frame.area());
    let geometry = workspace_geometry(main, app.sidebar.visible);
    render_editor(frame, geometry.editor, app, workspace);
    if let Some(tree) = geometry.tree {
        if geometry.tree_is_overlay {
            frame.render_widget(Clear, tree);
        }
        render_tree(frame, tree, workspace, geometry.tree_is_overlay);
    }

    render_shortcuts(frame, shortcuts, workspace.focus, shortcut_style);
    render_status(frame, status, app, workspace);
}

fn render_shortcuts(frame: &mut Frame<'_>, area: Rect, focus: Focus, style: ShortcutStyle) {
    let lines = vec![
        shortcut_line("Global", false, global_shortcuts(style)),
        shortcut_line(
            "Files",
            focus == Focus::Tree,
            "↑↓ Preview  →/Enter Folder  ← Parent  Enter Edit  n File  N Folder  r Rename  m Move  Del Delete",
        ),
        shortcut_line("Editor", focus == Focus::Editor, editor_shortcuts(style)),
    ];
    frame.render_widget(Paragraph::new(lines), area);
}

fn global_shortcuts(style: ShortcutStyle) -> &'static str {
    match style {
        ShortcutStyle::MacOs => {
            "⌘S Save  ^G Push  ⌘F Find  ⌘P Open  ⌘B Files  ⌘Z/⇧⌘Z Undo/Redo  ⌘C/X/V Clipboard  ⌘A All  ^Q Quit"
        }
        ShortcutStyle::Portable => {
            "^S Save  ^G Push  ^F Find  ^P Open  ^B Files  ^Z/Y Undo/Redo  ^C/X/V Clipboard  ^A All  ^Q Quit"
        }
    }
}

fn editor_shortcuts(style: ShortcutStyle) -> &'static str {
    match style {
        ShortcutStyle::MacOs => {
            "Esc Files  ⇧←→ Select  ⌥←→ Word  ⌘←→ Line  ⌘↑↓ Doc  ⌥⌫/Del Word Del  ⌘⌫/Del Line Del  Tab/S-Tab Indent"
        }
        ShortcutStyle::Portable => {
            "Esc Files  ⇧←→ Select  Alt←→ Word  Home/End Line  Alt⌫/Del Word Del  Tab/S-Tab Indent"
        }
    }
}

fn shortcut_line(label: &'static str, active: bool, shortcuts: &'static str) -> Line<'static> {
    let mut label_style = Style::default().add_modifier(Modifier::BOLD);
    if active {
        label_style = label_style.fg(Color::Black).bg(Color::Cyan);
    }
    Line::from(vec![
        Span::styled(format!("{label:<8}"), label_style),
        Span::raw(shortcuts),
    ])
}

fn render_tree(frame: &mut Frame<'_>, area: Rect, workspace: &WorkspaceState, overlay: bool) {
    let entries = visible_tree(&workspace.tree, &workspace.browser_directory);
    let viewport = selection_viewport(
        entries.len(),
        workspace.tree_selection,
        usize::from(area.height.saturating_sub(2)),
    );
    let items = entries
        .iter()
        .enumerate()
        .skip(viewport.start)
        .take(viewport.len())
        .map(|(index, entry)| {
            let selected = workspace.tree_selection == Some(index);
            let icon = match entry.kind {
                TreeEntryKind::Directory => "▸",
                TreeEntryKind::File if entry.enabled => "•",
                TreeEntryKind::File => "×",
                TreeEntryKind::Symlink => "↗",
            };
            let name = entry
                .path
                .file_name()
                .unwrap_or_else(|| entry.path.as_os_str())
                .to_string_lossy();
            let mut style = if entry.enabled {
                Style::default()
            } else {
                Style::default().fg(Color::DarkGray)
            };
            if selected {
                style = style.bg(Color::Blue).fg(Color::White);
            }
            ListItem::new(Line::styled(format!("{icon} {name}"), style))
        })
        .collect::<Vec<_>>();
    let focused = workspace.focus == Focus::Tree;
    let border_style = if focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let directory = if workspace.browser_directory.as_os_str().is_empty() {
        "/".to_owned()
    } else {
        workspace.browser_directory.display().to_string()
    };
    let title = if overlay {
        format!(" Files · {directory} · overlay ")
    } else {
        format!(" Files · {directory} ")
    };
    frame.render_widget(
        List::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(border_style)
                .title(title),
        ),
        area,
    );
}

fn render_editor(frame: &mut Frame<'_>, area: Rect, app: &App, workspace: &WorkspaceState) {
    let focused = workspace.focus == Focus::Editor;
    let border_style = if focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let mode = if focused { "Editing" } else { "Preview" };
    let title = workspace.current_note.as_deref().map_or_else(
        || format!(" {mode} "),
        |path| format!(" {mode} · {} ", path.display()),
    );
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(title);
    let inner = block.inner(area);
    let scroll = workspace
        .editor
        .as_ref()
        .map(|editor| editor_scroll(editor, inner))
        .unwrap_or_default();
    let highlights = workspace
        .editor
        .as_ref()
        .map(Editor::render_highlighted_spans)
        .unwrap_or_default();
    let content = workspace.editor.as_ref().map_or_else(
        || {
            let loading = matches!(
                app.pending_request,
                Some(crate::app::PendingRequest::LoadNote { .. })
            );
            Text::from(vec![
                Line::from(""),
                Line::from(Span::styled(
                    if loading {
                        "  Loading preview…"
                    } else {
                        "  Select a text file from Files to preview it."
                    },
                    Style::default().fg(Color::DarkGray),
                )),
            ])
        },
        |editor| editor_text(editor, &highlights),
    );
    frame.render_widget(Paragraph::new(content).scroll(scroll).block(block), area);
}

fn editor_scroll(editor: &Editor, viewport: Rect) -> (u16, u16) {
    let text = editor.text();
    let before = text.chars().take(editor.cursor()).collect::<String>();
    let cursor_line = before
        .chars()
        .filter(|character| *character == '\n')
        .count();
    let cursor_column = before
        .rsplit_once('\n')
        .map_or(before.as_str(), |(_, tail)| tail);
    let cursor_column = UnicodeWidthStr::width(cursor_column);
    let cursor_grapheme = text.chars().skip(editor.cursor()).collect::<String>();
    let cursor_width = cursor_grapheme
        .graphemes(true)
        .next()
        .filter(|grapheme| *grapheme != "\n")
        .map(UnicodeWidthStr::width)
        .unwrap_or(1)
        .max(1);
    let vertical = cursor_line.saturating_sub(viewport.height.saturating_sub(1).into());
    let horizontal = (cursor_column + cursor_width).saturating_sub(viewport.width.into());
    (
        u16::try_from(vertical).unwrap_or(u16::MAX),
        u16::try_from(horizontal).unwrap_or(u16::MAX),
    )
}

fn editor_text(editor: &Editor, highlights: &[HighlightSpan]) -> Text<'static> {
    let text = editor.text();
    let selection = editor.selection();
    let cursor = editor.cursor();
    let mut lines = vec![Line::default()];
    let mut index = 0;
    for grapheme in text.graphemes(true) {
        let grapheme_len = grapheme.chars().count();
        if grapheme == "\n" {
            if cursor == index {
                lines
                    .last_mut()
                    .expect("one line")
                    .push_span(cursor_span(" "));
            }
            lines.push(Line::default());
            index += grapheme_len;
            continue;
        }
        let style = grapheme_style(highlights, selection.as_ref(), cursor, index);
        let span = Span::styled(grapheme.to_owned(), style);
        lines.last_mut().expect("one line").push_span(span);
        index += grapheme_len;
    }
    if cursor == index {
        lines
            .last_mut()
            .expect("one line")
            .push_span(cursor_span(" "));
    }
    Text::from(lines)
}

fn grapheme_style(
    highlights: &[HighlightSpan],
    selection: Option<&std::ops::Range<usize>>,
    cursor: usize,
    index: usize,
) -> Style {
    let mut style = highlight_style_at(highlights, index);
    if selection.is_some_and(|selection| selection.contains(&index)) {
        style = style.bg(Color::Blue).fg(Color::White);
    }
    if cursor == index {
        style = cursor_style(style);
    }
    style
}

fn highlight_style_at(spans: &[HighlightSpan], index: usize) -> Style {
    spans
        .iter()
        .find(|span| span.range.contains(&index))
        .map(|span| to_ratatui_style(span.style))
        .unwrap_or_default()
}

fn to_ratatui_style(style: HighlightStyle) -> Style {
    let mut output = Style::default()
        .fg(Color::Rgb(
            style.foreground[0],
            style.foreground[1],
            style.foreground[2],
        ))
        .bg(Color::Rgb(
            style.background[0],
            style.background[1],
            style.background[2],
        ));
    if style.bold {
        output = output.add_modifier(Modifier::BOLD);
    }
    if style.italic {
        output = output.add_modifier(Modifier::ITALIC);
    }
    if style.underline {
        output = output.add_modifier(Modifier::UNDERLINED);
    }
    output
}

fn cursor_style(style: Style) -> Style {
    style.bg(Color::Yellow).fg(Color::Black)
}

fn cursor_span(text: &'static str) -> Span<'static> {
    Span::styled(text, cursor_style(Style::default()))
}

fn render_status(frame: &mut Frame<'_>, area: Rect, app: &App, workspace: &WorkspaceState) {
    let editor = workspace.editor.as_ref();
    let file_type = editor.map_or("No file", |editor| match editor.highlight_language() {
        HighlightLanguage::Markdown => "Markdown",
        HighlightLanguage::Html => "HTML",
        HighlightLanguage::PlainText => "Plain text",
    });
    let dirty = editor.is_some_and(Editor::is_dirty);
    let (line, column) = editor.map(cursor_position).unwrap_or((1, 1));
    let mutation = app
        .pending_mutation
        .as_ref()
        .map(|pending| match pending.kind {
            PendingMutationKind::Save { .. } => "saving",
            PendingMutationKind::RetryCommit => "retrying commit",
            PendingMutationKind::File(_) | PendingMutationKind::Delete => "applying mutation",
        });
    let request = app.pending_request.as_ref().map(|_| "loading");
    let commit = match &app.status.commit {
        CommitStatus::Idle => None,
        CommitStatus::Pending => Some("commit pending".to_owned()),
        CommitStatus::Committed { revision } => Some(format!(
            "committed {}",
            revision.chars().take(8).collect::<String>()
        )),
        CommitStatus::NoChanges => Some("saved · no Git changes".to_owned()),
        CommitStatus::SavedCommitFailed { message } => {
            Some(format!("saved · not committed: {message}"))
        }
    };
    let push = match &app.status.push {
        PushStatus::Idle => None,
        PushStatus::Pushing => Some("pushing".to_owned()),
        PushStatus::Pushed => Some("pushed".to_owned()),
        PushStatus::UpToDate => Some("remote up to date".to_owned()),
        PushStatus::Failed { message } => Some(message.clone()),
    };
    let mut parts = vec![
        file_type.to_owned(),
        if dirty { "modified" } else { "saved" }.to_owned(),
        format!("Ln {line}, Col {column}"),
    ];
    parts.extend(mutation.or(request).map(str::to_owned));
    parts.extend(push);
    parts.extend(commit);
    if let Some(message) = &app.status.message
        && !parts.iter().any(|part| part.contains(message))
    {
        parts.push(message.clone());
    }
    frame.render_widget(
        Paragraph::new(format!(" {} ", parts.join("  ·  ")))
            .style(Style::default().fg(Color::Black).bg(Color::Gray)),
        area,
    );
}

fn cursor_position(editor: &Editor) -> (usize, usize) {
    let text = editor.text();
    let before = text.chars().take(editor.cursor()).collect::<String>();
    let line = before
        .chars()
        .filter(|character| *character == '\n')
        .count()
        + 1;
    let column = before
        .rsplit_once('\n')
        .map_or(before.as_str(), |(_, tail)| tail);
    (line, UnicodeWidthStr::width(column) + 1)
}

struct VisibleTreeEntry {
    path: PathBuf,
    kind: TreeEntryKind,
    enabled: bool,
}

fn visible_tree(entries: &[TreeEntry], directory: &std::path::Path) -> Vec<VisibleTreeEntry> {
    crate::app::directory_entries(entries, directory)
        .iter()
        .map(|entry| VisibleTreeEntry {
            path: entry.path().to_path_buf(),
            kind: entry.kind(),
            enabled: entry.is_enabled(),
        })
        .collect()
}
