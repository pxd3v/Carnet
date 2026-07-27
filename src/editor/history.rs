#[derive(Clone)]
pub(super) struct Snapshot {
    pub(super) text: String,
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
}
