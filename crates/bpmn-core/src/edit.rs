//! Document mutations.
//!
//! Every mutation keeps the semantic model and the DI geometry consistent:
//! creating a node also creates its shape, deleting a node removes its shape
//! and every connected flow, moving shapes re-anchors the affected edges.
//! The editor and the MCP server both go through these methods.

use crate::di::{BpmnDiagram, BpmnEdge, BpmnPlane, BpmnShape, DiElement};
use crate::document::Document;
use crate::geometry::{Bounds, Point, Size};
use crate::ids::BpmnId;
use crate::model::{
    Activity, ActivityKind, BOSS_TARGET_NAMESPACE, BaseElement, Definitions, Event, EventPosition,
    FlowElement, FlowNode, Gateway, GatewayKind, Process, RootElement, SequenceFlow, TaskType,
};
use std::collections::HashSet;

/// What the palette can place in M2.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    /// A plain start event.
    StartEvent,
    /// A plain end event.
    EndEvent,
    /// A task of the given type.
    Task(TaskType),
    /// A gateway of the given kind.
    Gateway(GatewayKind),
}

impl NodeKind {
    /// Default shape size when placed, per BPMN conventions.
    pub fn default_size(self) -> Size {
        match self {
            NodeKind::StartEvent | NodeKind::EndEvent => Size::new(36.0, 36.0),
            NodeKind::Task(_) => Size::new(100.0, 80.0),
            NodeKind::Gateway(_) => Size::new(50.0, 50.0),
        }
    }

    fn id_prefix(self) -> &'static str {
        match self {
            NodeKind::StartEvent | NodeKind::EndEvent => "Event",
            NodeKind::Task(_) => "Activity",
            NodeKind::Gateway(_) => "Gateway",
        }
    }
}

/// The visual outline of a flow node, for border anchoring and hit testing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outline {
    /// Tasks (rounded corners are close enough to a rectangle).
    Rect,
    /// Events.
    Circle,
    /// Gateways.
    Diamond,
}

impl Outline {
    /// The point on this outline's border where a line from the shape center
    /// toward `toward` exits the shape.
    pub fn anchor(self, bounds: Bounds, toward: Point) -> Point {
        let center = bounds.center();
        let (dx, dy) = (toward.x - center.x, toward.y - center.y);
        if dx == 0.0 && dy == 0.0 {
            return center;
        }
        let (half_w, half_h) = (bounds.width / 2.0, bounds.height / 2.0);

        let t = match self {
            Outline::Rect => {
                let tx = if dx == 0.0 {
                    f64::INFINITY
                } else {
                    half_w / dx.abs()
                };
                let ty = if dy == 0.0 {
                    f64::INFINITY
                } else {
                    half_h / dy.abs()
                };
                tx.min(ty)
            }
            Outline::Circle => {
                let radius = half_w.min(half_h);
                radius / dx.hypot(dy)
            }
            Outline::Diamond => 1.0 / (dx.abs() / half_w + dy.abs() / half_h),
        };

        Point::new(center.x + dx * t, center.y + dy * t)
    }

    /// Whether `point` lies inside the outline placed at `bounds`.
    pub fn contains(self, bounds: Bounds, point: Point) -> bool {
        if !bounds.contains(point) {
            return false;
        }
        let center = bounds.center();
        let (dx, dy) = (point.x - center.x, point.y - center.y);
        let (half_w, half_h) = (bounds.width / 2.0, bounds.height / 2.0);
        match self {
            Outline::Rect => true,
            Outline::Circle => {
                let radius = half_w.min(half_h);
                dx.hypot(dy) <= radius
            }
            Outline::Diamond => dx.abs() / half_w + dy.abs() / half_h <= 1.0,
        }
    }
}

/// The outline of a modeled flow node.
pub fn outline_of(element: &FlowElement) -> Option<Outline> {
    match element {
        FlowElement::Event(_) => Some(Outline::Circle),
        FlowElement::Activity(_) => Some(Outline::Rect),
        FlowElement::Gateway(_) => Some(Outline::Diamond),
        FlowElement::SequenceFlow(_) | FlowElement::Unknown(_) => None,
    }
}

impl Document {
    /// A fresh, empty document: one executable-less process and its plane.
    pub fn new() -> Self {
        let process_id = BpmnId::new("Process_1");
        let process = Process {
            base: BaseElement::new(process_id.clone()),
            name: None,
            is_executable: Some(false),
            flow_elements: Vec::new(),
        };

        let definitions = Definitions {
            id: Some(BpmnId::new("Definitions_1")),
            target_namespace: String::from(BOSS_TARGET_NAMESPACE),
            ns_decls: crate::STANDARD_NS_DECLS
                .iter()
                .map(|(p, u)| (Some((*p).to_string()), (*u).to_string()))
                .collect(),
            root_elements: vec![RootElement::Process(process)],
            diagrams: vec![BpmnDiagram {
                id: Some(BpmnId::new("BPMNDiagram_1")),
                plane: Some(BpmnPlane {
                    id: Some(BpmnId::new("BPMNPlane_1")),
                    bpmn_element: Some(process_id),
                    ..BpmnPlane::default()
                }),
                ..BpmnDiagram::default()
            }],
            ..Definitions::default()
        };

        Self { definitions }
    }

    /// Every id used anywhere in the document.
    fn used_ids(&self) -> HashSet<String> {
        let mut ids = HashSet::new();
        let mut push = |id: Option<&BpmnId>| {
            if let Some(id) = id {
                ids.insert(id.as_str().to_string());
            }
        };

        push(self.definitions.id.as_ref());
        for root in &self.definitions.root_elements {
            if let RootElement::Process(process) = root {
                push(Some(&process.base.id));
                for element in &process.flow_elements {
                    push(element.id());
                }
            }
        }
        for diagram in &self.definitions.diagrams {
            push(diagram.id.as_ref());
            if let Some(plane) = &diagram.plane {
                push(plane.id.as_ref());
                for element in &plane.elements {
                    match element {
                        DiElement::Shape(s) => push(s.id.as_ref()),
                        DiElement::Edge(e) => push(e.id.as_ref()),
                        DiElement::Unknown(_) => {}
                    }
                }
            }
        }
        ids
    }

    /// A readable id that is unused in this document: `{prefix}_{n}`.
    pub fn fresh_id(&self, prefix: &str) -> BpmnId {
        let used = self.used_ids();
        let mut n = 1u32;
        loop {
            let candidate = format!("{prefix}_{n}");
            if !used.contains(&candidate) {
                return BpmnId::new(candidate.as_str());
            }
            n += 1;
        }
    }

    fn first_process_mut(&mut self) -> Option<&mut Process> {
        self.definitions
            .root_elements
            .iter_mut()
            .find_map(|root| match root {
                RootElement::Process(process) => Some(process),
                RootElement::Unknown(_) => None,
            })
    }

    fn plane_mut(&mut self) -> Option<&mut BpmnPlane> {
        self.definitions.diagrams.first_mut()?.plane.as_mut()
    }

    /// Creates a flow node of `kind` centered at `center`; returns its id.
    pub fn create_node(&mut self, kind: NodeKind, center: Point) -> Option<BpmnId> {
        let id = self.fresh_id(kind.id_prefix());
        let shape_id = BpmnId::new(format!("{id}_di").as_str());
        let size = kind.default_size();

        let node = FlowNode {
            base: BaseElement::new(id.clone()),
            ..FlowNode::default()
        };
        let element = match kind {
            NodeKind::StartEvent => FlowElement::Event(Event {
                node,
                position: EventPosition::Start {
                    is_interrupting: None,
                },
                definitions: Vec::new(),
            }),
            NodeKind::EndEvent => FlowElement::Event(Event {
                node,
                position: EventPosition::End,
                definitions: Vec::new(),
            }),
            NodeKind::Task(task_type) => FlowElement::Activity(Activity {
                node,
                kind: ActivityKind::Task(task_type),
            }),
            NodeKind::Gateway(gateway_kind) => FlowElement::Gateway(Gateway {
                node,
                kind: gateway_kind,
                default_flow: None,
                instantiate: None,
            }),
        };

        self.first_process_mut()?.flow_elements.push(element);
        self.plane_mut()?.elements.push(DiElement::Shape(BpmnShape {
            id: Some(shape_id),
            bpmn_element: id.clone(),
            bounds: Bounds::new(
                center.x - size.width / 2.0,
                center.y - size.height / 2.0,
                size.width,
                size.height,
            ),
            // The X marker on exclusive gateways, visible by convention.
            is_marker_visible: matches!(kind, NodeKind::Gateway(GatewayKind::Exclusive))
                .then_some(true),
            ..BpmnShape::default()
        }));

        Some(id)
    }

    /// Connects two flow nodes with a sequence flow; returns the flow's id.
    /// Refuses self-connections and unknown endpoints.
    pub fn connect(&mut self, source: &BpmnId, target: &BpmnId) -> Option<BpmnId> {
        if source == target {
            return None;
        }
        let (source_outline, source_bounds) = self.node_outline(source)?;
        let (target_outline, target_bounds) = self.node_outline(target)?;

        let id = self.fresh_id("Flow");
        let edge_id = BpmnId::new(format!("{id}_di").as_str());

        let from = source_outline.anchor(source_bounds, target_bounds.center());
        let to = target_outline.anchor(target_bounds, source_bounds.center());

        let process = self.first_process_mut()?;
        process
            .flow_elements
            .push(FlowElement::SequenceFlow(SequenceFlow {
                base: BaseElement::new(id.clone()),
                name: None,
                source: source.clone(),
                target: target.clone(),
                condition: None,
            }));

        for element in &mut process.flow_elements {
            if element.id() == Some(source) {
                if let Some(node) = flow_node_mut(element) {
                    node.outgoing.push(id.clone());
                }
            } else if element.id() == Some(target) {
                if let Some(node) = flow_node_mut(element) {
                    node.incoming.push(id.clone());
                }
            }
        }

        self.plane_mut()?.elements.push(DiElement::Edge(BpmnEdge {
            id: Some(edge_id),
            bpmn_element: id.clone(),
            waypoints: vec![from, to],
            ..BpmnEdge::default()
        }));

        Some(id)
    }

    /// The outline and DI bounds of a flow node.
    pub fn node_outline(&self, id: &BpmnId) -> Option<(Outline, Bounds)> {
        let index = self.flow_element_index();
        let outline = outline_of(index.get(id).copied()?)?;
        let plane = self.diagram()?.plane.as_ref()?;
        let bounds = plane.elements.iter().find_map(|element| match element {
            DiElement::Shape(shape) if shape.bpmn_element == *id => Some(shape.bounds),
            _ => None,
        })?;
        Some((outline, bounds))
    }

    /// Moves the shapes of `ids` by `(dx, dy)` and re-anchors affected edges.
    pub fn move_shapes(&mut self, ids: &HashSet<BpmnId>, dx: f64, dy: f64) {
        let Some(plane) = self.plane_mut() else {
            return;
        };
        for element in &mut plane.elements {
            match element {
                DiElement::Shape(shape) if ids.contains(&shape.bpmn_element) => {
                    shape.bounds.x += dx;
                    shape.bounds.y += dy;
                    if let Some(label) = &mut shape.label
                        && let Some(bounds) = &mut label.bounds
                    {
                        bounds.x += dx;
                        bounds.y += dy;
                    }
                }
                _ => {}
            }
        }
        self.reanchor_edges(ids);
    }

    /// Recomputes the endpoint waypoints of every edge touching `ids`.
    fn reanchor_edges(&mut self, ids: &HashSet<BpmnId>) {
        // Collect the flows to fix and their endpoints first (borrow split).
        let mut touched: Vec<(BpmnId, BpmnId, BpmnId)> = Vec::new();
        for element in self.flow_elements() {
            if let FlowElement::SequenceFlow(flow) = element
                && (ids.contains(&flow.source) || ids.contains(&flow.target))
            {
                touched.push((
                    flow.base.id.clone(),
                    flow.source.clone(),
                    flow.target.clone(),
                ));
            }
        }

        for (flow_id, source, target) in touched {
            let Some((source_outline, source_bounds)) = self.node_outline(&source) else {
                continue;
            };
            let Some((target_outline, target_bounds)) = self.node_outline(&target) else {
                continue;
            };
            let Some(plane) = self.plane_mut() else {
                return;
            };
            for element in &mut plane.elements {
                if let DiElement::Edge(edge) = element
                    && edge.bpmn_element == flow_id
                    && edge.waypoints.len() >= 2
                {
                    let n = edge.waypoints.len();
                    let toward_first = if n == 2 {
                        target_bounds.center()
                    } else {
                        edge.waypoints[1]
                    };
                    let toward_last = if n == 2 {
                        source_bounds.center()
                    } else {
                        edge.waypoints[n - 2]
                    };
                    edge.waypoints[0] = source_outline.anchor(source_bounds, toward_first);
                    edge.waypoints[n - 1] = target_outline.anchor(target_bounds, toward_last);
                }
            }
        }
    }

    /// Deletes elements (plus every sequence flow touching a deleted node) and
    /// their DI, and strips dangling references from surviving nodes.
    pub fn delete_elements(&mut self, ids: &HashSet<BpmnId>) {
        // Expand the set with flows that touch deleted nodes.
        let mut doomed: HashSet<BpmnId> = ids.clone();
        for element in self.flow_elements() {
            if let FlowElement::SequenceFlow(flow) = element
                && (doomed.contains(&flow.source) || doomed.contains(&flow.target))
            {
                doomed.insert(flow.base.id.clone());
            }
        }

        if let Some(process) = self.first_process_mut() {
            process
                .flow_elements
                .retain(|element| element.id().is_none_or(|id| !doomed.contains(id)));

            for element in &mut process.flow_elements {
                if let Some(node) = flow_node_mut(element) {
                    node.incoming.retain(|id| !doomed.contains(id));
                    node.outgoing.retain(|id| !doomed.contains(id));
                }
                if let FlowElement::Gateway(gateway) = element
                    && gateway
                        .default_flow
                        .as_ref()
                        .is_some_and(|id| doomed.contains(id))
                {
                    gateway.default_flow = None;
                }
            }
        }

        if let Some(plane) = self.plane_mut() {
            plane.elements.retain(|element| match element {
                DiElement::Shape(shape) => !doomed.contains(&shape.bpmn_element),
                DiElement::Edge(edge) => !doomed.contains(&edge.bpmn_element),
                DiElement::Unknown(_) => true,
            });
        }
    }
}

impl Default for Document {
    fn default() -> Self {
        Self::new()
    }
}

fn flow_node_mut(element: &mut FlowElement) -> Option<&mut FlowNode> {
    match element {
        FlowElement::Event(event) => Some(&mut event.node),
        FlowElement::Activity(activity) => Some(&mut activity.node),
        FlowElement::Gateway(gateway) => Some(&mut gateway.node),
        FlowElement::SequenceFlow(_) | FlowElement::Unknown(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn two_connected_nodes() -> (Document, BpmnId, BpmnId, BpmnId) {
        let mut doc = Document::new();
        let a = doc
            .create_node(NodeKind::StartEvent, Point::new(100.0, 100.0))
            .unwrap();
        let b = doc
            .create_node(NodeKind::Task(TaskType::User), Point::new(300.0, 100.0))
            .unwrap();
        let flow = doc.connect(&a, &b).unwrap();
        (doc, a, b, flow)
    }

    #[test]
    fn create_connect_and_ids_are_fresh() {
        let (doc, a, b, flow) = two_connected_nodes();
        assert_eq!(a.as_str(), "Event_1");
        assert_eq!(b.as_str(), "Activity_1");
        assert_eq!(flow.as_str(), "Flow_1");
        assert_eq!(doc.flow_elements().count(), 3);

        // Edge anchors sit on the shapes' borders, between the two centers.
        let plane = doc.diagram().unwrap().plane.as_ref().unwrap();
        let edge = plane
            .elements
            .iter()
            .find_map(|e| match e {
                DiElement::Edge(edge) => Some(edge),
                _ => None,
            })
            .unwrap();
        assert_eq!(edge.waypoints[0], Point::new(118.0, 100.0)); // circle right edge
        assert_eq!(edge.waypoints[1], Point::new(250.0, 100.0)); // task left edge

        // incoming/outgoing hints maintained.
        let index = doc.flow_element_index();
        let FlowElement::Event(start) = index[&a] else {
            panic!()
        };
        assert_eq!(start.node.outgoing, vec![flow.clone()]);
    }

    #[test]
    fn moving_reanchors_edges() {
        let (mut doc, _a, b, flow) = two_connected_nodes();
        let moved: HashSet<BpmnId> = [b].into_iter().collect();
        doc.move_shapes(&moved, 0.0, 200.0);

        let plane = doc.diagram().unwrap().plane.as_ref().unwrap();
        let edge = plane
            .elements
            .iter()
            .find_map(|e| match e {
                DiElement::Edge(edge) if edge.bpmn_element == flow => Some(edge),
                _ => None,
            })
            .unwrap();
        // Start anchor now points down-right toward the moved task.
        assert!(edge.waypoints[0].y > 100.0);
        // End anchor sits on the task border (its top edge faces the start).
        assert!((edge.waypoints[1].y - 260.0).abs() < 1e-9);
    }

    #[test]
    fn deleting_a_node_removes_connected_flows_and_di() {
        let (mut doc, a, b, _flow) = two_connected_nodes();
        let doomed: HashSet<BpmnId> = [a].into_iter().collect();
        doc.delete_elements(&doomed);

        assert_eq!(doc.flow_elements().count(), 1);
        let index = doc.flow_element_index();
        let FlowElement::Activity(task) = index[&b] else {
            panic!()
        };
        assert_eq!(task.node.incoming.len(), 0);

        let plane = doc.diagram().unwrap().plane.as_ref().unwrap();
        assert_eq!(plane.elements.len(), 1);
    }

    #[test]
    fn diamond_anchor_lands_on_the_diamond_edge() {
        let bounds = Bounds::new(0.0, 0.0, 50.0, 50.0);
        let anchor = Outline::Diamond.anchor(bounds, Point::new(100.0, 25.0));
        assert_eq!(anchor, Point::new(50.0, 25.0));
        let anchor = Outline::Diamond.anchor(bounds, Point::new(100.0, 100.0));
        // Along the diagonal the border sits halfway between center and corner.
        assert_eq!(anchor, Point::new(37.5, 37.5));
    }

    #[test]
    fn new_document_round_trips_without_a_process_body() {
        let doc = Document::new();
        assert!(doc.processes().next().is_some());
        assert!(doc.diagram().is_some());
    }
}
