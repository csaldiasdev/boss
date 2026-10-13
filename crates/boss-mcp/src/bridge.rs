//! The channel between MCP tool handlers and the editor.
//!
//! Tool handlers never touch editor state: they send a request and await a
//! oneshot reply. The app's update loop is the single writer — it drains
//! requests, applies them through the same edit session as user input (so
//! agent edits are undoable), and answers with snapshots or outcomes.

use bpmn_core::model::{EventDefinitionKind, LoopKind, TaskType};
use bpmn_core::{BpmnId, Document, NodeKind};
use tokio::sync::{mpsc, oneshot};

/// A node of a [`EditAction::CreateSequence`] chain.
#[derive(Debug)]
pub struct NodeSpec {
    /// What to create.
    pub kind: NodeKind,
    /// Optional label.
    pub name: Option<String>,
}

/// The optional fields of an [`EditAction::Update`]. The outer `Option` is
/// "touch this field at all"; a `Some(None)` inner value clears it.
#[derive(Debug, Default)]
pub struct UpdatePatch {
    /// Rename (`Some(None)` clears the name).
    pub name: Option<Option<String>>,
    /// Change a task's type.
    pub task_type: Option<TaskType>,
    /// Set or clear an event's trigger.
    pub event_trigger: Option<Option<EventDefinitionKind>>,
    /// Set or clear a gateway's default flow.
    pub gateway_default: Option<Option<BpmnId>>,
    /// Set or clear a sequence flow's condition expression.
    pub condition: Option<Option<String>>,
    /// Set or clear an activity's loop marker.
    pub loop_marker: Option<Option<LoopKind>>,
    /// Set or clear a call activity's called element.
    pub called_element: Option<Option<String>>,
    /// Expand or collapse a subprocess.
    pub expanded: Option<bool>,
}

/// A mutation an agent asked for.
#[derive(Debug)]
pub enum EditAction {
    /// Create a flow node. Placement, in priority order: explicit `(x, y)`
    /// center; right of `after` (auto-connected from it); inside
    /// `container`; else right of the existing content.
    CreateNode {
        /// What to create.
        kind: NodeKind,
        /// World/DI x of the center.
        x: Option<f64>,
        /// World/DI y of the center.
        y: Option<f64>,
        /// Place right of this node and connect from it.
        after: Option<BpmnId>,
        /// Place inside this subprocess.
        container: Option<BpmnId>,
        /// Optional label.
        name: Option<String>,
    },
    /// Create a connected left-to-right chain of nodes in one step.
    CreateSequence {
        /// The chain, in order.
        nodes: Vec<NodeSpec>,
        /// Start right of this node and connect from it.
        after: Option<BpmnId>,
        /// Build inside this subprocess.
        container: Option<BpmnId>,
    },
    /// Patch an element's properties (only the named fields change).
    Update {
        /// The element.
        id: BpmnId,
        /// What to change.
        patch: UpdatePatch,
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
    /// Create a pool (participant) centered at `(x, y)`.
    CreatePool {
        /// Optional pool label.
        name: Option<String>,
        /// World/DI x of the center.
        x: f64,
        /// World/DI y of the center.
        y: f64,
    },
    /// Append a lane to a pool.
    AddLane {
        /// The pool (participant id).
        pool: BpmnId,
        /// Optional lane label.
        name: Option<String>,
    },
    /// Connect two elements: a sequence flow inside a container, a message
    /// flow across pools.
    Connect {
        /// Source node id.
        source: BpmnId,
        /// Target node id.
        target: BpmnId,
        /// Optional flow label.
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
    /// New nodes and the flows that chain them.
    CreatedMany {
        /// The created nodes, in order.
        nodes: Vec<BpmnId>,
        /// The created flows, in order.
        flows: Vec<BpmnId>,
    },
    /// A patch applied these fields and refused those, with reasons.
    Updated {
        /// Field names that changed.
        applied: Vec<&'static str>,
        /// Field names that did not, and why.
        failed: Vec<(&'static str, String)>,
    },
    /// A bulk action touched `affected` and skipped `unknown` ids.
    Affected {
        /// The ids that existed and were acted on.
        affected: Vec<BpmnId>,
        /// The ids the document does not know.
        unknown: Vec<BpmnId>,
    },
    /// The document changed.
    Changed,
    /// Nothing to do (empty undo stack, empty selection, ...).
    Nothing,
}

impl ApplyOutcome {
    /// Whether the document actually changed — the predicate an undoable
    /// commit reports, so no-ops leave no undo step behind.
    pub fn changed_document(&self) -> bool {
        match self {
            Self::Nothing => false,
            Self::Affected { affected, .. } => !affected.is_empty(),
            Self::Updated { applied, .. } => !applied.is_empty(),
            Self::Created(_) | Self::CreatedMany { .. } | Self::Changed => true,
        }
    }
}

impl EditAction {
    /// Whether this action steps the editor's history instead of mutating
    /// the document; the app routes these to its edit session.
    pub fn is_history(&self) -> bool {
        matches!(self, Self::Undo | Self::Redo)
    }
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
