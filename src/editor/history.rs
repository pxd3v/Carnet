use super::buffer::TextBuffer;

#[derive(Clone)]
pub(super) struct Snapshot {
    pub(super) buffer: TextBuffer,
    pub(super) cursor: usize,
    pub(super) anchor: Option<usize>,
    pub(super) preferred_column: Option<usize>,
}

#[derive(Default)]
pub(super) struct History {
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
}

impl History {
    pub(super) fn record(&mut self, before: Snapshot) {
        self.undo.push(before);
        if self.undo.len() > super::EDITOR_HISTORY_ENTRY_LIMIT {
            // The first snapshot is the loaded baseline. Squash the oldest
            // intermediate state so complete undo always reaches that anchor.
            self.undo.remove(1);
        }
        self.redo.clear();
    }

    pub(super) fn undo(&mut self, current: Snapshot) -> Option<Snapshot> {
        let previous = self.undo.pop()?;
        self.redo.push(current);
        Some(previous)
    }

    pub(super) fn redo(&mut self, current: Snapshot) -> Option<Snapshot> {
        let next = self.redo.pop()?;
        self.undo.push(current);
        Some(next)
    }

    pub(super) fn entry_count(&self) -> usize {
        self.undo.len() + self.redo.len()
    }
}
