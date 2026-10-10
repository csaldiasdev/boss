//! Snapshot-based edit session: undo/redo and dirty tracking.
//!
//! Every committed mutation clones the whole document first. Snapshots are
//! drastically less bug-prone than inverse commands across dozens of element
//! kinds with DI side effects, documents are small (a clone is microseconds),
//! and the API hides the strategy so a delta-based undo can replace it later
//! without touching call sites.

use bpmn_core::Document;

/// Bounded undo depth.
const MAX_UNDO: usize = 100;

/// The open document plus its undo/redo history.
pub struct EditSession {
    document: Document,
    undo: Vec<Document>,
    redo: Vec<Document>,
    revision: u64,
    saved_revision: u64,
}

impl EditSession {
    /// Starts a session on `document` (considered saved).
    pub fn new(document: Document) -> Self {
        Self {
            document,
            undo: Vec::new(),
            redo: Vec::new(),
            revision: 0,
            saved_revision: 0,
        }
    }

    /// The current document.
    pub fn document(&self) -> &Document {
        &self.document
    }

    /// Whether the document changed since the last save.
    pub fn is_dirty(&self) -> bool {
        self.revision != self.saved_revision
    }

    /// Marks the current state as saved.
    pub fn mark_saved(&mut self) {
        self.saved_revision = self.revision;
    }

    /// Applies `mutate` as one undo step. When `mutate` returns `false` the
    /// document is restored and nothing is recorded.
    pub fn commit(&mut self, mutate: impl FnOnce(&mut Document) -> bool) {
        let snapshot = self.document.clone();
        if mutate(&mut self.document) {
            self.undo.push(snapshot);
            if self.undo.len() > MAX_UNDO {
                self.undo.remove(0);
            }
            self.redo.clear();
            self.revision += 1;
        } else {
            self.document = snapshot;
        }
    }

    /// Whether there is anything to undo.
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    /// Whether there is anything to redo.
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Steps one change back. Returns whether anything happened.
    pub fn undo(&mut self) -> bool {
        let Some(previous) = self.undo.pop() else {
            return false;
        };
        let current = std::mem::replace(&mut self.document, previous);
        self.redo.push(current);
        self.revision += 1;
        true
    }

    /// Steps one undone change forward. Returns whether anything happened.
    pub fn redo(&mut self) -> bool {
        let Some(next) = self.redo.pop() else {
            return false;
        };
        let current = std::mem::replace(&mut self.document, next);
        self.undo.push(current);
        self.revision += 1;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bpmn_core::NodeKind;
    use bpmn_core::geometry::Point;

    #[test]
    fn commit_undo_redo_and_dirty_tracking() {
        let mut session = EditSession::new(Document::new());
        assert!(!session.is_dirty());

        session.commit(|doc| {
            doc.create_node(NodeKind::StartEvent, Point::new(100.0, 100.0))
                .is_some()
        });
        assert!(session.is_dirty());
        assert_eq!(session.document().flow_elements().count(), 1);

        assert!(session.undo());
        assert_eq!(session.document().flow_elements().count(), 0);
        assert!(session.redo());
        assert_eq!(session.document().flow_elements().count(), 1);
        assert!(!session.redo());

        session.mark_saved();
        assert!(!session.is_dirty());
        assert!(session.undo());
        assert!(session.is_dirty());
    }

    #[test]
    fn failed_mutations_leave_no_trace() {
        let mut session = EditSession::new(Document::new());
        session.commit(|doc| {
            doc.connect(&"missing".into(), &"also_missing".into())
                .is_some()
        });
        assert!(!session.can_undo());
        assert!(!session.is_dirty());
    }
}
