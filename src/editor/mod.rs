mod buffer;
mod highlight;
mod history;
mod search;

pub use highlight::{HighlightLanguage, HighlightSpan, HighlightStyle};

use std::{
    ops::Range,
    sync::{Mutex, OnceLock},
};

use crate::workspace::{FileOperation, LoadedNote};
use buffer::TextBuffer;
use highlight::HighlightCache;
use history::{History, Snapshot};
use search::SearchState;
use thiserror::Error;

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum ClipboardError {
    #[error("clipboard is unavailable")]
    Unavailable,
}

pub trait Clipboard {
    fn read_text(&mut self) -> Result<String, ClipboardError>;
    fn write_text(&mut self, text: &str) -> Result<(), ClipboardError>;
}

struct SystemClipboard;

impl Clipboard for SystemClipboard {
    fn read_text(&mut self) -> Result<String, ClipboardError> {
        arboard::Clipboard::new()
            .and_then(|mut clipboard| clipboard.get_text())
            .map_err(|_| ClipboardError::Unavailable)
    }

    fn write_text(&mut self, text: &str) -> Result<(), ClipboardError> {
        arboard::Clipboard::new()
            .and_then(|mut clipboard| clipboard.set_text(text))
            .map_err(|_| ClipboardError::Unavailable)
    }
}

struct FallbackClipboard {
    primary: Box<dyn Clipboard>,
}

impl FallbackClipboard {
    fn new(primary: Box<dyn Clipboard>) -> Self {
        Self { primary }
    }

    fn local() -> &'static Mutex<String> {
        static LOCAL: OnceLock<Mutex<String>> = OnceLock::new();
        LOCAL.get_or_init(|| Mutex::new(String::new()))
    }
}

impl Clipboard for FallbackClipboard {
    fn read_text(&mut self) -> Result<String, ClipboardError> {
        match self.primary.read_text() {
            Ok(text) => {
                *Self::local()
                    .lock()
                    .unwrap_or_else(|error| error.into_inner()) = text.clone();
                Ok(text)
            }
            Err(_) => Ok(Self::local()
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .clone()),
        }
    }

    fn write_text(&mut self, text: &str) -> Result<(), ClipboardError> {
        *Self::local()
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = text.to_owned();
        let _ = self.primary.write_text(text);
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Motion {
    Left,
    Right,
    Up,
    Down,
    LineStart,
    LineEnd,
    DocumentStart,
    DocumentEnd,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EditorCommand {
    Move {
        motion: Motion,
        extend_selection: bool,
    },
    Insert(String),
    Backspace,
    Delete,
    Newline,
    BracketedPaste(String),
    Copy,
    Cut,
    Paste,
    SelectAll,
    Indent,
    Outdent,
    Undo,
    Redo,
    SetFindQuery(String),
    FindNext,
    FindPrevious,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EditorOutcome {
    NoChange,
    Moved,
    Changed,
    Copied,
    SearchMatch { current: usize, total: usize },
}

pub struct Editor {
    loaded: LoadedNote,
    buffer: TextBuffer,
    cursor: usize,
    anchor: Option<usize>,
    preferred_column: Option<usize>,
    history: History,
    clipboard: Box<dyn Clipboard>,
    search: SearchState,
    highlights: HighlightCache,
}

impl Editor {
    pub fn from_loaded(note: LoadedNote) -> Editor {
        Self::from_loaded_with_clipboard(note, Box::new(SystemClipboard))
    }

    pub fn from_loaded_with_clipboard(note: LoadedNote, clipboard: Box<dyn Clipboard>) -> Editor {
        let buffer = TextBuffer::new(note.text());
        let highlights = HighlightCache::for_path(note.path().relative());
        Self {
            loaded: note,
            buffer,
            cursor: 0,
            anchor: None,
            preferred_column: None,
            history: History::default(),
            clipboard: Box::new(FallbackClipboard::new(clipboard)),
            search: SearchState::default(),
            highlights,
        }
    }

    pub fn apply(&mut self, command: EditorCommand) -> EditorOutcome {
        match command {
            EditorCommand::Move {
                motion,
                extend_selection,
            } => self.move_cursor(motion, extend_selection),
            EditorCommand::Insert(text) => {
                let text = normalize_newlines(&text);
                self.transact(|editor| editor.replace_selection(&text))
            }
            EditorCommand::Backspace => self.transact(Self::backspace),
            EditorCommand::Delete => self.transact(Self::delete),
            EditorCommand::Newline => self.transact(|editor| editor.replace_selection("\n")),
            EditorCommand::BracketedPaste(text) => {
                let text = normalize_newlines(&text);
                self.transact(|editor| editor.replace_selection(&text))
            }
            EditorCommand::Copy => self.copy(),
            EditorCommand::Cut => self.cut(),
            EditorCommand::Paste => self.paste(),
            EditorCommand::SelectAll => self.select_all(),
            EditorCommand::Indent => self.transact(Self::indent),
            EditorCommand::Outdent => self.transact(Self::outdent),
            EditorCommand::Undo => self.undo(),
            EditorCommand::Redo => self.redo(),
            EditorCommand::SetFindQuery(query) => {
                self.search.set_query(query);
                EditorOutcome::NoChange
            }
            EditorCommand::FindNext => self.find(true),
            EditorCommand::FindPrevious => self.find(false),
        }
    }

    pub fn text(&self) -> String {
        self.buffer.text()
    }

    pub fn is_dirty(&self) -> bool {
        self.buffer.text() != self.loaded.text()
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn selection(&self) -> Option<Range<usize>> {
        self.anchor.and_then(|anchor| {
            (anchor != self.cursor).then(|| anchor.min(self.cursor)..anchor.max(self.cursor))
        })
    }

    pub fn selected_text(&self) -> Option<String> {
        self.selection().map(|range| self.buffer.slice(range))
    }

    pub fn highlight_language(&self) -> HighlightLanguage {
        self.highlights.language()
    }

    pub fn highlighted_spans(&mut self) -> &[HighlightSpan] {
        let text = self.buffer.text();
        self.highlights.spans(&text)
    }

    pub fn save_operation(&self, overwrite: bool) -> FileOperation {
        FileOperation::Save {
            note: self.loaded.clone(),
            content: self.text(),
            overwrite,
        }
    }

    pub(crate) fn accept_saved(&mut self, note: LoadedNote) {
        debug_assert_eq!(self.buffer.text(), note.text());
        self.loaded = note;
    }

    fn move_cursor(&mut self, motion: Motion, extend_selection: bool) -> EditorOutcome {
        let before = (self.cursor, self.selection());
        if !extend_selection {
            if let Some(selection) = self.selection() {
                match motion {
                    Motion::Left => {
                        self.cursor = selection.start;
                        self.anchor = None;
                        self.preferred_column = None;
                        return EditorOutcome::Moved;
                    }
                    Motion::Right => {
                        self.cursor = selection.end;
                        self.anchor = None;
                        self.preferred_column = None;
                        return EditorOutcome::Moved;
                    }
                    _ => {}
                }
            }
        } else if self.anchor.is_none() {
            self.anchor = Some(self.cursor);
        }

        self.cursor = match motion {
            Motion::Left => {
                self.preferred_column = None;
                self.buffer.previous_grapheme_boundary(self.cursor)
            }
            Motion::Right => {
                self.preferred_column = None;
                self.buffer.next_grapheme_boundary(self.cursor)
            }
            Motion::Up => {
                let (target, column) =
                    self.buffer
                        .vertical_target(self.cursor, -1, self.preferred_column);
                self.preferred_column = Some(column);
                target
            }
            Motion::Down => {
                let (target, column) =
                    self.buffer
                        .vertical_target(self.cursor, 1, self.preferred_column);
                self.preferred_column = Some(column);
                target
            }
            Motion::LineStart => {
                self.preferred_column = None;
                self.buffer.line_start(self.cursor)
            }
            Motion::LineEnd => {
                self.preferred_column = None;
                self.buffer.line_end(self.cursor)
            }
            Motion::DocumentStart => {
                self.preferred_column = None;
                0
            }
            Motion::DocumentEnd => {
                self.preferred_column = None;
                self.buffer.len_chars()
            }
        };
        if !extend_selection {
            self.anchor = None;
        }
        if before != (self.cursor, self.selection()) {
            self.search.reset_navigation();
        }
        if before == (self.cursor, self.selection()) {
            EditorOutcome::NoChange
        } else {
            EditorOutcome::Moved
        }
    }

    fn replace_selection(&mut self, text: &str) {
        let range = self.selection().unwrap_or(self.cursor..self.cursor);
        if range.is_empty() && text.is_empty() {
            return;
        }
        let start = range.start;
        self.buffer.replace(range, text);
        self.cursor = self
            .buffer
            .boundary_at_or_after(start + text.chars().count());
        self.anchor = None;
        self.preferred_column = None;
        self.search.reset_navigation();
    }

    fn backspace(&mut self) {
        if self.selection().is_some() {
            self.replace_selection("");
            return;
        }
        let start = self.buffer.previous_grapheme_boundary(self.cursor);
        if start == self.cursor {
            return;
        }
        self.buffer.replace(start..self.cursor, "");
        self.cursor = self.buffer.boundary_at_or_after(start);
        self.anchor = None;
        self.preferred_column = None;
    }

    fn delete(&mut self) {
        if self.selection().is_some() {
            self.replace_selection("");
            return;
        }
        let end = self.buffer.next_grapheme_boundary(self.cursor);
        if end == self.cursor {
            return;
        }
        self.buffer.replace(self.cursor..end, "");
        self.cursor = self.buffer.boundary_at_or_after(self.cursor);
        self.anchor = None;
        self.preferred_column = None;
    }

    fn copy(&mut self) -> EditorOutcome {
        let Some(text) = self.selected_text() else {
            return EditorOutcome::NoChange;
        };
        let _ = self.clipboard.write_text(&text);
        EditorOutcome::Copied
    }

    fn cut(&mut self) -> EditorOutcome {
        let Some(text) = self.selected_text() else {
            return EditorOutcome::NoChange;
        };
        let _ = self.clipboard.write_text(&text);
        self.transact(|editor| editor.replace_selection(""))
    }

    fn paste(&mut self) -> EditorOutcome {
        let Ok(text) = self.clipboard.read_text() else {
            return EditorOutcome::NoChange;
        };
        let text = normalize_newlines(&text);
        self.transact(|editor| editor.replace_selection(&text))
    }

    fn select_all(&mut self) -> EditorOutcome {
        let before = (self.cursor, self.selection());
        self.anchor = Some(0);
        self.cursor = self.buffer.len_chars();
        self.preferred_column = None;
        if before == (self.cursor, self.selection()) {
            EditorOutcome::NoChange
        } else {
            EditorOutcome::Moved
        }
    }

    fn indent(&mut self) {
        let starts = self.selected_line_starts();
        for start in starts.iter().rev() {
            self.buffer.replace(*start..*start, "    ");
        }
        self.cursor += starts.iter().filter(|start| **start <= self.cursor).count() * 4;
        if let Some(anchor) = &mut self.anchor {
            *anchor += starts.iter().filter(|start| **start <= *anchor).count() * 4;
        }
        self.snap_endpoints();
        self.preferred_column = None;
    }

    fn outdent(&mut self) {
        let removals: Vec<_> = self
            .selected_line_starts()
            .into_iter()
            .filter_map(|start| {
                let end = self.buffer.line_end(start);
                let line = self.buffer.slice(start..end);
                let count = if line.starts_with('\t') {
                    1
                } else {
                    line.chars()
                        .take(4)
                        .take_while(|character| *character == ' ')
                        .count()
                };
                (count > 0).then_some((start, count))
            })
            .collect();
        for (start, count) in removals.iter().rev() {
            self.buffer.replace(*start..*start + *count, "");
        }
        self.cursor = adjusted_after_removals(self.cursor, &removals);
        if let Some(anchor) = &mut self.anchor {
            *anchor = adjusted_after_removals(*anchor, &removals);
        }
        self.snap_endpoints();
        self.preferred_column = None;
    }

    fn snap_endpoints(&mut self) {
        match self.anchor {
            None => {
                self.cursor = self.buffer.boundary_at_or_after(self.cursor);
            }
            Some(anchor) if anchor < self.cursor => {
                self.anchor = Some(self.buffer.boundary_at_or_before(anchor));
                self.cursor = self.buffer.boundary_at_or_after(self.cursor);
            }
            Some(anchor) if anchor > self.cursor => {
                self.anchor = Some(self.buffer.boundary_at_or_after(anchor));
                self.cursor = self.buffer.boundary_at_or_before(self.cursor);
            }
            Some(_) => {
                self.cursor = self.buffer.boundary_at_or_after(self.cursor);
                self.anchor = Some(self.cursor);
            }
        }
    }

    fn selected_line_starts(&self) -> Vec<usize> {
        let selection = self.selection();
        let (start, end) = selection
            .as_ref()
            .map_or((self.cursor, self.cursor), |range| (range.start, range.end));
        let start_line = self.buffer.char_to_line(start);
        let mut end_line = self.buffer.char_to_line(end);
        if selection.is_some()
            && end > start
            && end == self.buffer.line_to_char(end_line)
            && end_line > start_line
        {
            end_line -= 1;
        }
        (start_line..=end_line)
            .map(|line| self.buffer.line_to_char(line))
            .collect()
    }

    fn transact(&mut self, edit: impl FnOnce(&mut Self)) -> EditorOutcome {
        let before = self.snapshot();
        edit(self);
        if self.buffer.text() != before.text {
            self.history.record(before);
            self.search.reset_navigation();
            EditorOutcome::Changed
        } else if self.cursor != before.cursor || self.anchor != before.anchor {
            EditorOutcome::Moved
        } else {
            EditorOutcome::NoChange
        }
    }

    fn undo(&mut self) -> EditorOutcome {
        let current = self.snapshot();
        let Some(previous) = self.history.undo(current) else {
            return EditorOutcome::NoChange;
        };
        self.restore(previous);
        EditorOutcome::Changed
    }

    fn redo(&mut self) -> EditorOutcome {
        let current = self.snapshot();
        let Some(next) = self.history.redo(current) else {
            return EditorOutcome::NoChange;
        };
        self.restore(next);
        EditorOutcome::Changed
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            text: self.buffer.text(),
            cursor: self.cursor,
            anchor: self.anchor,
            preferred_column: self.preferred_column,
        }
    }

    fn restore(&mut self, snapshot: Snapshot) {
        self.buffer = TextBuffer::new(&snapshot.text);
        self.cursor = snapshot.cursor;
        self.anchor = snapshot.anchor;
        self.preferred_column = snapshot.preferred_column;
        self.search.reset_navigation();
    }

    fn find(&mut self, forward: bool) -> EditorOutcome {
        let text = self.buffer.text();
        let Some(found) = self.search.navigate(&text, self.cursor, forward) else {
            return EditorOutcome::NoChange;
        };
        self.anchor = Some(found.range.start);
        self.cursor = found.range.end;
        self.preferred_column = None;
        EditorOutcome::SearchMatch {
            current: found.current,
            total: found.total,
        }
    }
}

fn normalize_newlines(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

fn adjusted_after_removals(position: usize, removals: &[(usize, usize)]) -> usize {
    let removed_before_position: usize = removals
        .iter()
        .map(|(start, count)| position.saturating_sub(*start).min(*count))
        .sum();
    position - removed_before_position
}
