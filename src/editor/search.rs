use std::{collections::HashSet, ops::Range};

use unicode_segmentation::UnicodeSegmentation;

pub(super) struct SearchMatch {
    pub(super) range: Range<usize>,
    pub(super) current: usize,
    pub(super) total: usize,
}

#[derive(Default)]
pub(super) struct SearchState {
    query: String,
    current: Option<usize>,
}

impl SearchState {
    pub(super) fn set_query(&mut self, query: String) {
        self.query = query;
        self.current = None;
    }

    pub(super) fn reset_navigation(&mut self) {
        self.current = None;
    }

    pub(super) fn navigate(
        &mut self,
        text: &str,
        cursor: usize,
        forward: bool,
    ) -> Option<SearchMatch> {
        let matches = literal_matches(text, &self.query);
        if matches.is_empty() {
            self.current = None;
            return None;
        }
        let current = match self.current {
            Some(current) if forward => (current + 1) % matches.len(),
            Some(current) => (current + matches.len() - 1) % matches.len(),
            None if forward => matches
                .iter()
                .position(|range| range.start >= cursor)
                .unwrap_or(0),
            None => matches
                .iter()
                .rposition(|range| range.end <= cursor)
                .unwrap_or(matches.len() - 1),
        };
        self.current = Some(current);
        Some(SearchMatch {
            range: matches[current].clone(),
            current: current + 1,
            total: matches.len(),
        })
    }
}

fn literal_matches(text: &str, query: &str) -> Vec<Range<usize>> {
    if query.is_empty() {
        return Vec::new();
    }
    let boundaries: HashSet<_> = text
        .grapheme_indices(true)
        .map(|(byte, _)| byte)
        .chain(std::iter::once(text.len()))
        .collect();
    text.match_indices(query)
        .filter_map(|(start, matched)| {
            let end = start + matched.len();
            if boundaries.contains(&start) && boundaries.contains(&end) {
                Some(text[..start].chars().count()..text[..end].chars().count())
            } else {
                None
            }
        })
        .collect()
}
