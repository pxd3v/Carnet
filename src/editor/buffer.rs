use ropey::Rope;
use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

#[derive(Clone)]
pub(super) struct TextBuffer {
    rope: Rope,
}

impl TextBuffer {
    pub(super) fn new(text: &str) -> Self {
        Self {
            rope: Rope::from_str(text),
        }
    }

    pub(super) fn text(&self) -> String {
        self.rope.to_string()
    }

    pub(super) fn len_chars(&self) -> usize {
        self.rope.len_chars()
    }

    pub(super) fn slice(&self, range: Range<usize>) -> String {
        self.rope.slice(range).to_string()
    }

    pub(super) fn replace(&mut self, range: Range<usize>, text: &str) -> bool {
        if range.is_empty() {
            if text.is_empty() {
                return false;
            }
        } else if self.rope.slice(range.clone()).chars().eq(text.chars()) {
            return false;
        }
        let start = range.start;
        if !range.is_empty() {
            self.rope.remove(range);
        }
        if !text.is_empty() {
            self.rope.insert(start, text);
        }
        true
    }

    pub(super) fn previous_grapheme_boundary(&self, char_index: usize) -> usize {
        self.grapheme_boundaries()
            .into_iter()
            .take_while(|boundary| *boundary < char_index)
            .last()
            .unwrap_or(0)
    }

    pub(super) fn next_grapheme_boundary(&self, char_index: usize) -> usize {
        self.grapheme_boundaries()
            .into_iter()
            .find(|boundary| *boundary > char_index)
            .unwrap_or_else(|| self.len_chars())
    }

    pub(super) fn previous_word_start(&self, char_index: usize) -> usize {
        let text = self.rope.to_string();
        text.unicode_word_indices()
            .map(|(byte, _)| text[..byte].chars().count())
            .rfind(|start| *start < char_index)
            .unwrap_or(0)
    }

    pub(super) fn next_word_end(&self, char_index: usize) -> usize {
        let text = self.rope.to_string();
        text.unicode_word_indices()
            .map(|(byte, word)| text[..byte + word.len()].chars().count())
            .find(|end| *end > char_index)
            .unwrap_or_else(|| self.len_chars())
    }

    pub(super) fn boundary_at_or_after(&self, char_index: usize) -> usize {
        self.grapheme_boundaries()
            .into_iter()
            .find(|boundary| *boundary >= char_index)
            .unwrap_or_else(|| self.len_chars())
    }

    pub(super) fn boundary_at_or_before(&self, char_index: usize) -> usize {
        self.grapheme_boundaries()
            .into_iter()
            .take_while(|boundary| *boundary <= char_index)
            .last()
            .unwrap_or(0)
    }

    pub(super) fn line_start(&self, char_index: usize) -> usize {
        self.rope.line_to_char(self.rope.char_to_line(char_index))
    }

    pub(super) fn char_to_line(&self, char_index: usize) -> usize {
        self.rope.char_to_line(char_index)
    }

    pub(super) fn line_to_char(&self, line_index: usize) -> usize {
        self.rope.line_to_char(line_index)
    }

    pub(super) fn line_end(&self, char_index: usize) -> usize {
        let line = self.rope.char_to_line(char_index);
        let start = self.rope.line_to_char(line);
        let mut end = start + self.rope.line(line).len_chars();
        if end > start && self.rope.char(end - 1) == '\n' {
            end -= 1;
        }
        end
    }

    pub(super) fn vertical_target(
        &self,
        char_index: usize,
        line_delta: isize,
        preferred_column: Option<usize>,
    ) -> (usize, usize) {
        let current_line = self.rope.char_to_line(char_index);
        let target_line = current_line
            .saturating_add_signed(line_delta)
            .min(self.rope.len_lines().saturating_sub(1));
        let column = preferred_column.unwrap_or_else(|| {
            let start = self.rope.line_to_char(current_line);
            UnicodeWidthStr::width(self.rope.slice(start..char_index).to_string().as_str())
        });
        let start = self.rope.line_to_char(target_line);
        let end = self.line_end(start);
        let line = self.rope.slice(start..end).to_string();
        let mut width = 0;
        let mut target = start;
        for grapheme in line.graphemes(true) {
            let next_width = width + UnicodeWidthStr::width(grapheme);
            if next_width > column {
                break;
            }
            width = next_width;
            target += grapheme.chars().count();
        }
        (target, column)
    }

    fn grapheme_boundaries(&self) -> Vec<usize> {
        let text = self.rope.to_string();
        let mut boundaries = Vec::with_capacity(text.graphemes(true).count() + 1);
        boundaries.push(0);
        boundaries.extend(
            text.grapheme_indices(true)
                .skip(1)
                .map(|(byte, _)| text[..byte].chars().count()),
        );
        boundaries.push(text.chars().count());
        boundaries.sort_unstable();
        boundaries.dedup();
        boundaries
    }
}
