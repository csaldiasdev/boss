//! The channel between MCP tool handlers and the editor.
//!
//! Tool handlers never touch editor state: they send a request and await a
//! oneshot reply. The app's update loop is the single writer — it drains
//! requests, applies them through the same edit session as user input (so
//! agent edits are undoable), and answers with snapshots or outcomes.

use bpmn_core::{BpmnId, Document, NodeKind};
use tokio::sync::{mpsc, oneshot};

/// A mutation an agent asked for.
#[derive(Debug)]
pub enum EditAction {
    /// Create a flow node centered at `(x, y)`.
    CreateNode {
        /// What to create.
        kind: NodeKind,
        /// World/DI x of the center.
        x: f64,
        /// World/DI y of the center.
        y: f64,
        /// Optional label.
        name: Option<String>,
    },
    /// Create a boundary event on `host`'s border, toward `(x, y)`.
    CreateBoundary {
        /// The host activity.
        host: BpmnId,
        /// World/DI x the event should face.
        x: f64,
        /// World/DI y the event should face.
        y: f64,
    },
    /// Connect two nodes with a sequence flow.
    Connect {
        /// Source node id.
        source: BpmnId,
        /// Target node id.
        target: BpmnId,
        /// Optional flow label.
        name: Option<String>,
    },
    /// Rename an element (`None` clears the name).
    SetName {
        /// The element.
        id: BpmnId,
        /// The new name.
        name: Option<String>,
    },
    /// Delete elements (connected flows go with them).
    Delete {
        /// The elements to delete.
        ids: Vec<BpmnId>,
    },
    /// Move shapes by a delta.
    Move {
        /// The shapes to move.
        ids: Vec<BpmnId>,
        /// Horizontal delta.
        dx: f64,
        /// Vertical delta.
        dy: f64,
    },
    /// Replace the whole document (load XML).
    Replace {
        /// The parsed replacement.
        document: Box<Document>,
    },
    /// Step one change back.
    Undo,
    /// Step one undone change forward.
    Redo,
}

/// What applying an action produced.
#[derive(Debug, Clone)]
pub enum ApplyOutcome {
    /// A new element with this id.
    Created(BpmnId),
    /// The document changed.
    Changed,
    /// Nothing to do (empty undo stack, empty selection, ...).
    Nothing,
}

/// A request traveling from a tool handler to the editor.
#[derive(Debug)]
pub enum EditorRequest {
    /// A read: answer with a snapshot of the current document.
    Snapshot {
        /// Where the snapshot goes.
        reply: oneshot::Sender<Document>,
    },
    /// A write: apply the action as one undoable step.
    Apply {
        /// What to do.
        action: EditAction,
        /// Where the outcome goes (`Err` is a human-readable refusal).
        reply: oneshot::Sender<Result<ApplyOutcome, String>>,
    },
}

/// The tool handlers' end of the bridge.
#[derive(Clone)]
pub struct EditorHandle {
    tx: mpsc::Sender<EditorRequest>,
}

/// Creates the bridge; the receiver side belongs to the app's update loop.
pub fn channel() -> (EditorHandle, mpsc::Receiver<EditorRequest>) {
    let (tx, rx) = mpsc::channel(32);
    (EditorHandle { tx }, rx)
}

const GONE: &str = "the boss editor is not responding";

impl EditorHandle {
    /// A snapshot of the current document.
    pub async fn snapshot(&self) -> Result<Document, String> {
        let (reply, answer) = oneshot::channel();
        self.tx
            .send(EditorRequest::Snapshot { reply })
            .await
            .map_err(|_| GONE.to_string())?;
        answer.await.map_err(|_| GONE.to_string())
    }

    /// Applies one action as one undoable step.
    pub async fn apply(&self, action: EditAction) -> Result<ApplyOutcome, String> {
        let (reply, answer) = oneshot::channel();
        self.tx
            .send(EditorRequest::Apply { action, reply })
            .await
            .map_err(|_| GONE.to_string())?;
        answer.await.map_err(|_| GONE.to_string())?
    }
}
