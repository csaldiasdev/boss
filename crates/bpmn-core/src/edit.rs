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
    Activity, ActivityKind, BOSS_TARGET_NAMESPACE, BaseElement, Condition, Definitions, Event,
    EventPosition, FlowElement, FlowNode, Gateway, GatewayKind, LoopKind, LoopMarker, Process,
    RootElement, SequenceFlow, TaskType,
};
use std::collections::HashSet;

/// What the palette can place in M2.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    /// A plain start event.
    StartEvent,
    /// A plain end event.
    EndEvent,
    /// A plain intermediate catch event.
    IntermediateCatch,
    /// A plain intermediate throw event.
    IntermediateThrow,
    /// A boundary event (requires a host activity under the cursor).
    Boundary,
    /// A task of the given type.
    Task(TaskType),
    /// A gateway of the given kind.
    Gateway(GatewayKind),
}

impl NodeKind {
    /// Default shape size when placed, per BPMN conventions.
    pub fn default_size(self) -> Size {
        match self {
            NodeKind::StartEvent
            | NodeKind::EndEvent
            | NodeKind::IntermediateCatch
            | NodeKind::IntermediateThrow
            | NodeKind::Boundary => Size::new(36.0, 36.0),
            NodeKind::Task(_) => Size::new(100.0, 80.0),
            NodeKind::Gateway(_) => Size::new(50.0, 50.0),
        }
    }

    fn id_prefix(self) -> &'static str {
        match self {
            NodeKind::StartEvent
            | NodeKind::EndEvent
            | NodeKind::IntermediateCatch
            | NodeKind::IntermediateThrow
            | NodeKind::Boundary => "Event",
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
            NodeKind::IntermediateCatch => FlowElement::Event(Event {
                node,
                position: EventPosition::IntermediateCatch,
                definitions: Vec::new(),
            }),
            NodeKind::IntermediateThrow => FlowElement::Event(Event {
                node,
                position: EventPosition::IntermediateThrow,
                definitions: Vec::new(),
            }),
            // Boundary events go through create_boundary_event (need a host).
            NodeKind::Boundary => return None,
            NodeKind::Task(task_type) => FlowElement::Activity(Activity {
                node,
                kind: ActivityKind::Task(task_type),
                loop_marker: None,
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

    /// Creates a boundary event attached to `host`, sitting on its border at
    /// the point nearest to `toward`.
    pub fn create_boundary_event(&mut self, host: &BpmnId, toward: Point) -> Option<BpmnId> {
        let (outline, bounds) = self.node_outline(host)?;
        if !matches!(
            self.flow_element_index().get(host),
            Some(FlowElement::Activity(_))
        ) {
            return None;
        }
        let center = outline.anchor(bounds, toward);

        let id = self.fresh_id("Event");
        let shape_id = BpmnId::new(format!("{id}_di").as_str());
        let element = FlowElement::Event(Event {
            node: FlowNode {
                base: BaseElement::new(id.clone()),
                ..FlowNode::default()
            },
            position: EventPosition::Boundary {
                attached_to: host.clone(),
                cancel_activity: None,
            },
            definitions: Vec::new(),
        });

        self.first_process_mut()?.flow_elements.push(element);
        self.plane_mut()?.elements.push(DiElement::Shape(BpmnShape {
            id: Some(shape_id),
            bpmn_element: id.clone(),
            bounds: Bounds::new(center.x - 18.0, center.y - 18.0, 36.0, 36.0),
            ..BpmnShape::default()
        }));
        Some(id)
    }

    /// Sets an event's (single) trigger; `None` makes it a plain event. A
    /// definition whose body we only hold verbatim is replaced wholesale.
    pub fn set_event_definition(
        &mut self,
        id: &BpmnId,
        kind: Option<crate::model::EventDefinitionKind>,
    ) -> bool {
        use crate::model::{EventDefinition, EventDefinitionBody, EventDefinitionKind};
        let Some(FlowElement::Event(event)) = self.flow_element_mut(id) else {
            return false;
        };
        event.definitions = match kind {
            None => Vec::new(),
            Some(EventDefinitionKind::Message) => vec![EventDefinition {
                kind: EventDefinitionKind::Message,
                body: EventDefinitionBody::Message {
                    id: None,
                    message_ref: None,
                },
            }],
            Some(kind) => vec![EventDefinition {
                kind,
                body: EventDefinitionBody::Empty { id: None },
            }],
        };
        true
    }

    /// Sets an activity's loop marker; `None` clears it. A marker holding
    /// verbatim content is replaced wholesale.
    pub fn set_loop_marker(&mut self, id: &BpmnId, kind: Option<LoopKind>) -> bool {
        let Some(FlowElement::Activity(activity)) = self.flow_element_mut(id) else {
            return false;
        };
        activity.loop_marker = kind.map(|kind| LoopMarker { kind, raw: None });
        true
    }

    /// The boundary events attached to any of `hosts`.
    pub fn attached_boundaries(&self, hosts: &HashSet<BpmnId>) -> Vec<BpmnId> {
        self.flow_elements()
            .filter_map(|element| match element {
                FlowElement::Event(event) => match &event.position {
                    EventPosition::Boundary { attached_to, .. } if hosts.contains(attached_to) => {
                        Some(event.node.base.id.clone())
                    }
                    _ => None,
                },
                _ => None,
            })
            .collect()
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
    /// Boundary events follow their host; a boundary moved on its own is
    /// clamped back onto its host's border.
    pub fn move_shapes(&mut self, ids: &HashSet<BpmnId>, dx: f64, dy: f64) {
        let mut moving: HashSet<BpmnId> = ids.clone();
        moving.extend(self.attached_boundaries(ids));

        {
            let Some(plane) = self.plane_mut() else {
                return;
            };
            for element in &mut plane.elements {
                match element {
                    DiElement::Shape(shape) if moving.contains(&shape.bpmn_element) => {
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
        }
        self.clamp_boundaries(&moving, ids);
        self.reanchor_edges(&moving);
    }

    /// Clamps moved boundary events back onto their host borders, unless the
    /// host moved too (then the boundary already followed it rigidly).
    fn clamp_boundaries(&mut self, moved: &HashSet<BpmnId>, explicit: &HashSet<BpmnId>) {
        let mut clamps: Vec<(BpmnId, Point)> = Vec::new();
        for id in moved {
            let Some(FlowElement::Event(event)) = self.flow_element_index().get(id).copied() else {
                continue;
            };
            let EventPosition::Boundary { attached_to, .. } = &event.position else {
                continue;
            };
            // Host moved along: boundary keeps its relative spot.
            if explicit.contains(attached_to) || moved.contains(attached_to) {
                continue;
            }
            let Some((outline, host_bounds)) = self.node_outline(attached_to) else {
                continue;
            };
            let Some((_, own_bounds)) = self.node_outline(id) else {
                continue;
            };
            clamps.push((id.clone(), outline.anchor(host_bounds, own_bounds.center())));
        }

        if clamps.is_empty() {
            return;
        }
        let Some(plane) = self.plane_mut() else {
            return;
        };
        for (id, center) in clamps {
            for element in &mut plane.elements {
                if let DiElement::Shape(shape) = element
                    && shape.bpmn_element == id
                {
                    shape.bounds.x = center.x - shape.bounds.width / 2.0;
                    shape.bounds.y = center.y - shape.bounds.height / 2.0;
                }
            }
        }
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
            self.reanchor_flow(&flow_id, &source, &target);
        }
    }

    /// Recomputes one flow's endpoint waypoints against its nodes' borders.
    fn reanchor_flow(&mut self, flow_id: &BpmnId, source: &BpmnId, target: &BpmnId) {
        let Some((source_outline, source_bounds)) = self.node_outline(source) else {
            return;
        };
        let Some((target_outline, target_bounds)) = self.node_outline(target) else {
            return;
        };
        let Some(plane) = self.plane_mut() else {
            return;
        };
        for element in &mut plane.elements {
            if let DiElement::Edge(edge) = element
                && edge.bpmn_element == *flow_id
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

impl Document {
    fn flow_element_mut(&mut self, id: &BpmnId) -> Option<&mut FlowElement> {
        self.definitions
            .root_elements
            .iter_mut()
            .filter_map(|root| match root {
                RootElement::Process(process) => Some(process),
                RootElement::Unknown(_) => None,
            })
            .flat_map(|process| process.flow_elements.iter_mut())
            .find(|element| element.id() == Some(id))
    }

    /// Renames a flow element (`None` removes the name).
    pub fn set_name(&mut self, id: &BpmnId, name: Option<String>) -> bool {
        let name = name.filter(|n| !n.is_empty());
        match self.flow_element_mut(id) {
            Some(FlowElement::Event(e)) => e.node.name = name,
            Some(FlowElement::Activity(a)) => a.node.name = name,
            Some(FlowElement::Gateway(g)) => g.node.name = name,
            Some(FlowElement::SequenceFlow(f)) => f.name = name,
            _ => return false,
        }
        true
    }

    /// Changes a task's type (its XML tag).
    pub fn set_task_type(&mut self, id: &BpmnId, task_type: TaskType) -> bool {
        if let Some(FlowElement::Activity(activity)) = self.flow_element_mut(id) {
            activity.kind = ActivityKind::Task(task_type);
            true
        } else {
            false
        }
    }

    /// Sets a gateway's default flow; `None` clears it. The flow must leave
    /// that gateway.
    pub fn set_gateway_default(&mut self, id: &BpmnId, default: Option<BpmnId>) -> bool {
        if let Some(d) = &default
            && !self.outgoing_flows(id).contains(d)
        {
            return false;
        }
        if let Some(FlowElement::Gateway(gateway)) = self.flow_element_mut(id) {
            gateway.default_flow = default;
            true
        } else {
            false
        }
    }

    /// The sequence flows leaving a node, in document order.
    pub fn outgoing_flows(&self, id: &BpmnId) -> Vec<BpmnId> {
        self.flow_elements()
            .filter_map(|element| match element {
                FlowElement::SequenceFlow(flow) if flow.source == *id => Some(flow.base.id.clone()),
                _ => None,
            })
            .collect()
    }

    /// Sets a sequence flow's condition text. Empty/`None` clears it; an
    /// existing `xsi:type` is kept when only the text changes.
    pub fn set_condition(&mut self, id: &BpmnId, text: Option<String>) -> bool {
        let Some(FlowElement::SequenceFlow(flow)) = self.flow_element_mut(id) else {
            return false;
        };
        let text = text.filter(|t| !t.is_empty());
        flow.condition = match (text, flow.condition.take()) {
            (None, _) => None,
            (Some(text), Some(Condition::Expression { xsi_type, .. })) => {
                Some(Condition::Expression { xsi_type, text })
            }
            (Some(text), _) => Some(Condition::Expression {
                xsi_type: None,
                text,
            }),
        };
        true
    }

    /// Replaces a flow's waypoints (at least two), then re-anchors the ends.
    pub fn set_edge_waypoints(&mut self, flow_id: &BpmnId, waypoints: &[Point]) -> bool {
        if waypoints.len() < 2 {
            return false;
        }
        let (source, target) = match self.flow_element_index().get(flow_id) {
            Some(FlowElement::SequenceFlow(flow)) => (flow.source.clone(), flow.target.clone()),
            _ => return false,
        };

        let Some(plane) = self.plane_mut() else {
            return false;
        };
        let mut found = false;
        for element in &mut plane.elements {
            if let DiElement::Edge(edge) = element
                && edge.bpmn_element == *flow_id
            {
                edge.waypoints = waypoints.to_vec();
                found = true;
            }
        }
        if found {
            self.reanchor_flow(flow_id, &source, &target);
        }
        found
    }

    /// Copies `selection` (plus flows fully inside it) into a standalone
    /// document — the clipboard payload is plain BPMN XML.
    #[must_use]
    pub fn extract(&self, selection: &HashSet<BpmnId>) -> Document {
        let mut out = Document::new();

        let keep: HashSet<BpmnId> = self
            .flow_elements()
            .filter_map(|element| match element {
                FlowElement::SequenceFlow(flow) => (selection.contains(&flow.source)
                    && selection.contains(&flow.target))
                .then(|| flow.base.id.clone()),
                other => other.id().filter(|id| selection.contains(id)).cloned(),
            })
            .collect();

        if let Some(process) = out.first_process_mut() {
            // (populated below; split for borrow clarity)
            process.flow_elements.clear();
        }
        let mut elements = Vec::new();
        for element in self.flow_elements() {
            if element.id().is_some_and(|id| keep.contains(id)) {
                let mut element = element.clone();
                if let Some(node) = flow_node_mut(&mut element) {
                    node.incoming.retain(|id| keep.contains(id));
                    node.outgoing.retain(|id| keep.contains(id));
                }
                if let FlowElement::Gateway(gateway) = &mut element
                    && gateway
                        .default_flow
                        .as_ref()
                        .is_some_and(|id| !keep.contains(id))
                {
                    gateway.default_flow = None;
                }
                elements.push(element);
            }
        }
        if let Some(process) = out.first_process_mut() {
            process.flow_elements = elements;
        }

        let mut di = Vec::new();
        if let Some(plane) = self.diagram().and_then(|d| d.plane.as_ref()) {
            for element in &plane.elements {
                match element {
                    DiElement::Shape(shape) if keep.contains(&shape.bpmn_element) => {
                        di.push(DiElement::Shape(shape.clone()));
                    }
                    DiElement::Edge(edge) if keep.contains(&edge.bpmn_element) => {
                        di.push(DiElement::Edge(edge.clone()));
                    }
                    _ => {}
                }
            }
        }
        if let Some(plane) = out.plane_mut() {
            plane.elements = di;
        }

        out
    }

    /// Pastes every modeled flow element of `source` into this document with
    /// fresh ids, offset by `(dx, dy)`. Returns the new node ids (not flows).
    #[expect(
        clippy::too_many_lines,
        reason = "one coherent remap pass; splitting it obscures the id mapping"
    )]
    pub fn paste_from(&mut self, source: &Document, dx: f64, dy: f64) -> Vec<BpmnId> {
        use std::collections::HashMap;

        let mut used = self.used_ids();
        let mut fresh = |prefix: &str| -> BpmnId {
            let mut n = 1u32;
            loop {
                let candidate = format!("{prefix}_{n}");
                if !used.contains(&candidate) {
                    used.insert(candidate.clone());
                    return BpmnId::new(candidate.as_str());
                }
                n += 1;
            }
        };

        // First pass: a new id for every modeled element.
        let mut mapping: HashMap<BpmnId, BpmnId> = HashMap::new();
        for element in source.flow_elements() {
            let Some(old) = element.id() else { continue };
            let prefix = match element {
                FlowElement::Event(_) => "Event",
                FlowElement::Activity(_) => "Activity",
                FlowElement::Gateway(_) => "Gateway",
                FlowElement::SequenceFlow(_) => "Flow",
                FlowElement::Unknown(_) => continue,
            };
            mapping.insert(old.clone(), fresh(prefix));
        }
        let remap = |id: &BpmnId, mapping: &HashMap<BpmnId, BpmnId>| {
            mapping.get(id).cloned().unwrap_or_else(|| id.clone())
        };

        let mut new_nodes = Vec::new();
        let mut elements = Vec::new();
        for element in source.flow_elements() {
            let Some(old) = element.id() else { continue };
            let Some(new) = mapping.get(old) else {
                continue;
            };
            let mut element = element.clone();
            match &mut element {
                FlowElement::SequenceFlow(flow) => {
                    // A flow whose endpoints were not pasted makes no sense.
                    if !mapping.contains_key(&flow.source) || !mapping.contains_key(&flow.target) {
                        continue;
                    }
                    flow.base.id = new.clone();
                    flow.source = remap(&flow.source, &mapping);
                    flow.target = remap(&flow.target, &mapping);
                }
                other => {
                    if let Some(node) = flow_node_mut(other) {
                        node.base.id = new.clone();
                        node.incoming = node
                            .incoming
                            .iter()
                            .filter(|id| mapping.contains_key(*id))
                            .map(|id| remap(id, &mapping))
                            .collect();
                        node.outgoing = node
                            .outgoing
                            .iter()
                            .filter(|id| mapping.contains_key(*id))
                            .map(|id| remap(id, &mapping))
                            .collect();
                    }
                    if let FlowElement::Gateway(gateway) = other {
                        gateway.default_flow = gateway
                            .default_flow
                            .take()
                            .filter(|id| mapping.contains_key(id))
                            .map(|id| remap(&id, &mapping));
                    }
                    new_nodes.push(new.clone());
                }
            }
            elements.push(element);
        }
        if let Some(process) = self.first_process_mut() {
            process.flow_elements.extend(elements);
        }

        let mut di = Vec::new();
        if let Some(plane) = source.diagram().and_then(|d| d.plane.as_ref()) {
            for element in &plane.elements {
                match element {
                    DiElement::Shape(shape) if mapping.contains_key(&shape.bpmn_element) => {
                        let mut shape = shape.clone();
                        shape.bpmn_element = remap(&shape.bpmn_element, &mapping);
                        shape.id = Some(BpmnId::new(format!("{}_di", shape.bpmn_element).as_str()));
                        shape.bounds.x += dx;
                        shape.bounds.y += dy;
                        if let Some(label) = &mut shape.label {
                            shape_label_offset(label, dx, dy);
                        }
                        di.push(DiElement::Shape(shape));
                    }
                    DiElement::Edge(edge) if mapping.contains_key(&edge.bpmn_element) => {
                        let mut edge = edge.clone();
                        edge.bpmn_element = remap(&edge.bpmn_element, &mapping);
                        edge.id = Some(BpmnId::new(format!("{}_di", edge.bpmn_element).as_str()));
                        for point in &mut edge.waypoints {
                            point.x += dx;
                            point.y += dy;
                        }
                        if let Some(label) = &mut edge.label {
                            shape_label_offset(label, dx, dy);
                        }
                        di.push(DiElement::Edge(edge));
                    }
                    _ => {}
                }
            }
        }
        if let Some(plane) = self.plane_mut() {
            plane.elements.extend(di);
        }

        new_nodes
    }
}

fn shape_label_offset(label: &mut crate::di::BpmnLabel, dx: f64, dy: f64) {
    if let Some(bounds) = &mut label.bounds {
        bounds.x += dx;
        bounds.y += dy;
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

#[cfg(test)]
mod m3_tests {
    use super::*;

    #[test]
    fn property_setters() {
        let mut doc = Document::new();
        let gw = doc
            .create_node(
                NodeKind::Gateway(GatewayKind::Exclusive),
                Point::new(0.0, 0.0),
            )
            .unwrap();
        let task = doc
            .create_node(NodeKind::Task(TaskType::Generic), Point::new(200.0, 0.0))
            .unwrap();
        let flow = doc.connect(&gw, &task).unwrap();

        assert!(doc.set_name(&task, Some(String::from("Review"))));
        assert!(doc.set_task_type(&task, TaskType::User));
        assert!(doc.set_gateway_default(&gw, Some(flow.clone())));
        assert!(
            !doc.set_gateway_default(&gw, Some(task.clone())),
            "not an outgoing flow"
        );
        assert!(doc.set_condition(&flow, Some(String::from("${ok}"))));

        let index = doc.flow_element_index();
        let FlowElement::Activity(a) = index[&task] else {
            panic!()
        };
        assert_eq!(a.node.name.as_deref(), Some("Review"));
        assert_eq!(a.kind, ActivityKind::Task(TaskType::User));
        let FlowElement::SequenceFlow(f) = index[&flow] else {
            panic!()
        };
        assert!(matches!(
            &f.condition,
            Some(Condition::Expression { xsi_type: None, text }) if text == "${ok}"
        ));

        // Clearing works too.
        assert!(doc.set_condition(&flow, None));
        assert!(doc.set_name(&task, None));
    }

    #[test]
    fn waypoints_replace_and_reanchor() {
        let mut doc = Document::new();
        let a = doc
            .create_node(NodeKind::StartEvent, Point::new(0.0, 0.0))
            .unwrap();
        let b = doc
            .create_node(NodeKind::EndEvent, Point::new(300.0, 0.0))
            .unwrap();
        let flow = doc.connect(&a, &b).unwrap();

        assert!(doc.set_edge_waypoints(
            &flow,
            &[
                Point::new(0.0, 0.0),
                Point::new(150.0, 120.0),
                Point::new(300.0, 0.0),
            ],
        ));

        let plane = doc.diagram().unwrap().plane.as_ref().unwrap();
        let edge = plane
            .elements
            .iter()
            .find_map(|e| match e {
                DiElement::Edge(edge) => Some(edge),
                _ => None,
            })
            .unwrap();
        assert_eq!(edge.waypoints.len(), 3);
        assert_eq!(edge.waypoints[1], Point::new(150.0, 120.0));
        // Endpoints re-anchored toward the interior waypoint, so they left
        // the shape centers and sit on the circles' borders.
        assert!((edge.waypoints[0].x.hypot(edge.waypoints[0].y) - 18.0).abs() < 1e-6);
    }

    #[test]
    fn extract_and_paste_remint_ids_and_offset() {
        let mut doc = Document::new();
        let a = doc
            .create_node(NodeKind::StartEvent, Point::new(100.0, 100.0))
            .unwrap();
        let b = doc
            .create_node(NodeKind::Task(TaskType::User), Point::new(300.0, 100.0))
            .unwrap();
        let _flow = doc.connect(&a, &b).unwrap();

        let selection: HashSet<BpmnId> = [a.clone(), b.clone()].into_iter().collect();
        let clipboard = doc.extract(&selection);
        assert_eq!(clipboard.flow_elements().count(), 3);

        let pasted = doc.paste_from(&clipboard, 20.0, 20.0);
        assert_eq!(pasted.len(), 2);
        assert_eq!(doc.flow_elements().count(), 6);

        // Fresh ids, no collisions.
        let index = doc.flow_element_index();
        assert_eq!(index.len(), 6);
        assert!(index.contains_key(&BpmnId::from("Event_2")));
        assert!(index.contains_key(&BpmnId::from("Activity_2")));
        assert!(index.contains_key(&BpmnId::from("Flow_2")));

        // The pasted flow connects the pasted nodes, not the originals.
        let FlowElement::SequenceFlow(f) = index[&BpmnId::from("Flow_2")] else {
            panic!()
        };
        assert_eq!(f.source.as_str(), "Event_2");
        assert_eq!(f.target.as_str(), "Activity_2");

        // And the pasted shape is offset.
        let (_, bounds) = doc.node_outline(&BpmnId::from("Event_2")).unwrap();
        assert_eq!(bounds.center(), Point::new(120.0, 120.0));
    }

    #[test]
    fn extract_skips_flows_leaving_the_selection() {
        let mut doc = Document::new();
        let a = doc
            .create_node(NodeKind::StartEvent, Point::new(0.0, 0.0))
            .unwrap();
        let b = doc
            .create_node(NodeKind::EndEvent, Point::new(300.0, 0.0))
            .unwrap();
        doc.connect(&a, &b).unwrap();

        let selection: HashSet<BpmnId> = [a].into_iter().collect();
        let clipboard = doc.extract(&selection);
        assert_eq!(clipboard.flow_elements().count(), 1, "flow left behind");
    }
}
