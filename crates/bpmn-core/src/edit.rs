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
    Activity, ActivityKind, BOSS_TARGET_NAMESPACE, BaseElement, CallActivity, Condition,
    Definitions, Event, EventPosition, FlowElement, FlowNode, Gateway, GatewayKind, LoopKind,
    LoopMarker, Process, RootElement, SequenceFlow, SubProcess, SubProcessKind, TaskType,
};
use std::collections::HashSet;

/// What the palette can place.
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
    /// An empty subprocess of the given flavor, placed expanded.
    SubProcess(SubProcessKind),
    /// A call activity.
    CallActivity,
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
            NodeKind::Task(_) | NodeKind::CallActivity => Size::new(100.0, 80.0),
            NodeKind::Gateway(_) => Size::new(50.0, 50.0),
            NodeKind::SubProcess(_) => Size::new(350.0, 200.0),
        }
    }

    fn id_prefix(self) -> &'static str {
        match self {
            NodeKind::StartEvent
            | NodeKind::EndEvent
            | NodeKind::IntermediateCatch
            | NodeKind::IntermediateThrow
            | NodeKind::Boundary => "Event",
            NodeKind::Task(_) | NodeKind::SubProcess(_) | NodeKind::CallActivity => "Activity",
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
            }
        }
        for element in self.flow_elements() {
            push(element.id());
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

    /// Mutable access to the flow elements of `container`: a process id or an
    /// expanded subprocess id.
    fn elements_of_mut(&mut self, container: &BpmnId) -> Option<&mut Vec<FlowElement>> {
        fn scan<'a>(
            container: &BpmnId,
            elements: &'a mut Vec<FlowElement>,
        ) -> Option<&'a mut Vec<FlowElement>> {
            for element in elements {
                let FlowElement::Activity(activity) = element else {
                    continue;
                };
                let is_it = activity.node.base.id == *container;
                if let ActivityKind::SubProcess(sub) = &mut activity.kind {
                    if is_it {
                        return Some(&mut sub.flow_elements);
                    }
                    if let Some(found) = scan(container, &mut sub.flow_elements) {
                        return Some(found);
                    }
                }
            }
            None
        }

        for root in &mut self.definitions.root_elements {
            let RootElement::Process(process) = root else {
                continue;
            };
            if process.base.id == *container {
                return Some(&mut process.flow_elements);
            }
            if let Some(found) = scan(container, &mut process.flow_elements) {
                return Some(found);
            }
        }
        None
    }

    /// The deepest *expanded* subprocess whose shape contains `point`, falling
    /// back to the first process: where a node dropped at `point` belongs.
    /// `except` (a node plus its descendants) never counts as a container, so
    /// a subprocess cannot be dropped into itself.
    fn container_at(&self, point: Point, except: &HashSet<BpmnId>) -> Option<BpmnId> {
        let index = self.flow_element_index();
        let plane = self.diagram()?.plane.as_ref()?;
        let hidden = self.hidden_by_collapse();

        let mut best: Option<(BpmnId, f64)> = None;
        for element in &plane.elements {
            let DiElement::Shape(shape) = element else {
                continue;
            };
            let id = &shape.bpmn_element;
            if except.contains(id) || hidden.contains(id) || shape.is_expanded != Some(true) {
                continue;
            }
            let Some(FlowElement::Activity(activity)) = index.get(id).copied() else {
                continue;
            };
            if !matches!(activity.kind, ActivityKind::SubProcess(_))
                || !shape.bounds.contains(point)
            {
                continue;
            }
            let area = shape.bounds.width * shape.bounds.height;
            if best.as_ref().is_none_or(|(_, a)| area < *a) {
                best = Some((id.clone(), area));
            }
        }

        best.map(|(id, _)| id)
            .or_else(|| self.processes().next().map(|p| p.base.id.clone()))
    }

    /// Creates a flow node of `kind` centered at `center`; returns its id.
    /// The node lands in the expanded subprocess under `center`, if any.
    pub fn create_node(&mut self, kind: NodeKind, center: Point) -> Option<BpmnId> {
        let id = self.fresh_id(kind.id_prefix());
        let shape_id = BpmnId::new(format!("{id}_di").as_str());
        let size = kind.default_size();
        let container = self.container_at(center, &HashSet::new())?;

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
            NodeKind::SubProcess(sub_kind) => FlowElement::Activity(Activity {
                node,
                kind: ActivityKind::SubProcess(SubProcess {
                    kind: sub_kind,
                    flow_elements: Vec::new(),
                }),
                loop_marker: None,
            }),
            NodeKind::CallActivity => FlowElement::Activity(Activity {
                node,
                kind: ActivityKind::CallActivity(CallActivity::default()),
                loop_marker: None,
            }),
            NodeKind::Gateway(gateway_kind) => FlowElement::Gateway(Gateway {
                node,
                kind: gateway_kind,
                default_flow: None,
                instantiate: None,
            }),
        };

        self.elements_of_mut(&container)?.push(element);
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
            // Subprocesses start expanded so their body can be edited.
            is_expanded: matches!(kind, NodeKind::SubProcess(_)).then_some(true),
            ..BpmnShape::default()
        }));

        Some(id)
    }

    /// Connects two flow nodes with a sequence flow; returns the flow's id.
    /// Refuses self-connections, unknown endpoints, and endpoints living in
    /// different containers (the spec keeps sequence flows inside one).
    pub fn connect(&mut self, source: &BpmnId, target: &BpmnId) -> Option<BpmnId> {
        if source == target {
            return None;
        }
        let container = self.container_of(source)?;
        if self.container_of(target)? != container {
            return None;
        }
        let (source_outline, source_bounds) = self.node_outline(source)?;
        let (target_outline, target_bounds) = self.node_outline(target)?;

        let id = self.fresh_id("Flow");
        let edge_id = BpmnId::new(format!("{id}_di").as_str());

        let from = source_outline.anchor(source_bounds, target_bounds.center());
        let to = target_outline.anchor(target_bounds, source_bounds.center());

        self.elements_of_mut(&container)?
            .push(FlowElement::SequenceFlow(SequenceFlow {
                base: BaseElement::new(id.clone()),
                name: None,
                source: source.clone(),
                target: target.clone(),
                condition: None,
            }));

        if let Some(node) = self.flow_element_mut(source).and_then(flow_node_mut) {
            node.outgoing.push(id.clone());
        }
        if let Some(node) = self.flow_element_mut(target).and_then(flow_node_mut) {
            node.incoming.push(id.clone());
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
        let container = self.container_of(host)?;

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

        self.elements_of_mut(&container)?.push(element);
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
    /// Boundary events follow their host, subprocess children follow their
    /// container, and a boundary moved on its own is clamped back onto its
    /// host's border. A node dropped into (or out of) an expanded subprocess
    /// is reparented to the container under its new position.
    pub fn move_shapes(&mut self, ids: &HashSet<BpmnId>, dx: f64, dy: f64) {
        let mut moving: HashSet<BpmnId> = ids.clone();
        for id in ids {
            moving.extend(self.descendants_of(id));
        }
        moving.extend(self.attached_boundaries(&moving));

        // Edges fully inside the moving set travel rigidly (all waypoints);
        // edges with one end inside get their endpoints re-anchored.
        let mut rigid: HashSet<BpmnId> = HashSet::new();
        let mut touched: Vec<(BpmnId, BpmnId, BpmnId)> = Vec::new();
        for element in self.flow_elements() {
            if let FlowElement::SequenceFlow(flow) = element {
                match (moving.contains(&flow.source), moving.contains(&flow.target)) {
                    (true, true) => {
                        rigid.insert(flow.base.id.clone());
                    }
                    (true, false) | (false, true) => touched.push((
                        flow.base.id.clone(),
                        flow.source.clone(),
                        flow.target.clone(),
                    )),
                    (false, false) => {}
                }
            }
        }

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
                    DiElement::Edge(edge) if rigid.contains(&edge.bpmn_element) => {
                        for point in &mut edge.waypoints {
                            point.x += dx;
                            point.y += dy;
                        }
                        if let Some(label) = &mut edge.label
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
        for (flow_id, source, target) in touched {
            self.reanchor_flow(&flow_id, &source, &target);
        }
        self.reparent_moved(ids);
    }

    /// Reparents explicitly moved nodes whose center now sits in a different
    /// container, then relocates any flow whose endpoints agree on a container
    /// the flow is not in. Cross-container flows are left for validation.
    fn reparent_moved(&mut self, ids: &HashSet<BpmnId>) {
        let mut relocations: Vec<(BpmnId, BpmnId)> = Vec::new();
        for id in ids {
            let movable = match self.flow_element_index().get(id) {
                // Boundaries stick to their host; flows follow their endpoints.
                Some(FlowElement::Event(event)) => {
                    !matches!(event.position, EventPosition::Boundary { .. })
                }
                Some(FlowElement::Activity(_) | FlowElement::Gateway(_)) => true,
                _ => false,
            };
            if !movable {
                continue;
            }
            let Some((_, bounds)) = self.node_outline(id) else {
                continue;
            };
            let mut except: HashSet<BpmnId> = self.descendants_of(id).into_iter().collect();
            except.insert(id.clone());
            let Some(new) = self.container_at(bounds.center(), &except) else {
                continue;
            };
            if self.container_of(id).is_some_and(|old| old != new) {
                relocations.push((id.clone(), new));
            }
        }
        if relocations.is_empty() {
            return;
        }

        for (id, to) in &relocations {
            self.relocate(id, to);
            let host: HashSet<BpmnId> = [id.clone()].into_iter().collect();
            for boundary in self.attached_boundaries(&host) {
                self.relocate(&boundary, to);
            }
        }

        // A flow belongs with its endpoints when they agree on a container.
        let mut flow_moves: Vec<(BpmnId, BpmnId)> = Vec::new();
        for element in self.flow_elements() {
            if let FlowElement::SequenceFlow(flow) = element
                && let Some(sc) = self.container_of(&flow.source)
                && self.container_of(&flow.target) == Some(sc.clone())
                && self.container_of(&flow.base.id) != Some(sc.clone())
            {
                flow_moves.push((flow.base.id.clone(), sc));
            }
        }
        for (id, to) in flow_moves {
            self.relocate(&id, &to);
        }
    }

    /// Moves one flow element from its current container into `to`.
    fn relocate(&mut self, id: &BpmnId, to: &BpmnId) {
        let Some(from) = self.container_of(id) else {
            return;
        };
        if from == *to {
            return;
        }
        let Some(elements) = self.elements_of_mut(&from) else {
            return;
        };
        let Some(position) = elements.iter().position(|e| e.id() == Some(id)) else {
            return;
        };
        let element = elements.remove(position);
        match self.elements_of_mut(to) {
            Some(target) => target.push(element),
            // The destination vanished: put the element back rather than lose it.
            None => {
                if let Some(back) = self.elements_of_mut(&from) {
                    back.push(element);
                }
            }
        }
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

    /// Collapses or expands a subprocess. Collapsing shrinks the shape to task
    /// size (children stay in the model but are hidden); expanding resizes it
    /// around its children again.
    pub fn set_subprocess_expanded(&mut self, id: &BpmnId, expanded: bool) -> bool {
        const COLLAPSED: Size = Size {
            width: 100.0,
            height: 80.0,
        };
        const PADDING: f64 = 40.0;

        match self.flow_element_index().get(id) {
            Some(FlowElement::Activity(activity))
                if matches!(activity.kind, ActivityKind::SubProcess(_)) => {}
            _ => return false,
        }

        // The expanded size wraps the children; an empty body gets the
        // default placement size.
        let mut child_box: Option<Bounds> = None;
        if expanded {
            let children: HashSet<BpmnId> = self.descendants_of(id).into_iter().collect();
            if let Some(plane) = self.diagram().and_then(|d| d.plane.as_ref()) {
                for element in &plane.elements {
                    if let DiElement::Shape(shape) = element
                        && children.contains(&shape.bpmn_element)
                    {
                        child_box = Some(match child_box {
                            None => shape.bounds,
                            Some(a) => a.union(shape.bounds),
                        });
                    }
                }
            }
        }

        let mut changed = false;
        {
            let Some(plane) = self.plane_mut() else {
                return false;
            };
            for element in &mut plane.elements {
                let DiElement::Shape(shape) = element else {
                    continue;
                };
                if shape.bpmn_element != *id {
                    continue;
                }
                shape.is_expanded = Some(expanded);
                if expanded {
                    let size = NodeKind::SubProcess(SubProcessKind::Embedded).default_size();
                    shape.bounds = child_box.map_or_else(
                        || Bounds::new(shape.bounds.x, shape.bounds.y, size.width, size.height),
                        |b| {
                            Bounds::new(
                                b.x - PADDING,
                                b.y - PADDING,
                                b.width + PADDING * 2.0,
                                b.height + PADDING * 2.0,
                            )
                        },
                    );
                } else {
                    shape.bounds.width = COLLAPSED.width;
                    shape.bounds.height = COLLAPSED.height;
                }
                changed = true;
            }
        }
        if changed {
            let moved: HashSet<BpmnId> = [id.clone()].into_iter().collect();
            let boundaries: HashSet<BpmnId> =
                self.attached_boundaries(&moved).into_iter().collect();
            self.clamp_boundaries(&boundaries, &HashSet::new());
            let mut touching = moved;
            touching.extend(boundaries);
            self.reanchor_edges(&touching);
        }
        changed
    }

    /// Sets a call activity's `calledElement`; empty/`None` clears it.
    pub fn set_called_element(&mut self, id: &BpmnId, called: Option<String>) -> bool {
        let Some(FlowElement::Activity(activity)) = self.flow_element_mut(id) else {
            return false;
        };
        let ActivityKind::CallActivity(call) = &mut activity.kind else {
            return false;
        };
        call.called_element = called.filter(|c| !c.is_empty());
        true
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

    /// Deletes elements (plus the contents of deleted subprocesses and every
    /// sequence flow touching a deleted node) and their DI, and strips
    /// dangling references from surviving nodes.
    pub fn delete_elements(&mut self, ids: &HashSet<BpmnId>) {
        // Expand the set with subprocess contents, then with flows that touch
        // deleted nodes.
        let mut doomed: HashSet<BpmnId> = ids.clone();
        for id in ids {
            doomed.extend(self.descendants_of(id));
        }
        for element in self.flow_elements() {
            if let FlowElement::SequenceFlow(flow) = element
                && (doomed.contains(&flow.source) || doomed.contains(&flow.target))
            {
                doomed.insert(flow.base.id.clone());
            }
        }

        for root in &mut self.definitions.root_elements {
            if let RootElement::Process(process) = root {
                prune_deleted(&mut process.flow_elements, &doomed);
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
        fn scan<'a>(elements: &'a mut [FlowElement], id: &BpmnId) -> Option<&'a mut FlowElement> {
            for element in elements {
                if element.id() == Some(id) {
                    return Some(element);
                }
                if let FlowElement::Activity(activity) = element
                    && let ActivityKind::SubProcess(sub) = &mut activity.kind
                    && let Some(found) = scan(&mut sub.flow_elements, id)
                {
                    return Some(found);
                }
            }
            None
        }

        self.definitions
            .root_elements
            .iter_mut()
            .filter_map(|root| match root {
                RootElement::Process(process) => Some(process),
                RootElement::Unknown(_) => None,
            })
            .find_map(|process| scan(&mut process.flow_elements, id))
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

    /// Changes a task's type (its XML tag). Subprocesses and call activities
    /// are not tasks and keep their kind.
    pub fn set_task_type(&mut self, id: &BpmnId, task_type: TaskType) -> bool {
        if let Some(FlowElement::Activity(activity)) = self.flow_element_mut(id)
            && matches!(activity.kind, ActivityKind::Task(_))
        {
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

    /// Copies `selection` (plus flows fully inside it, plus the contents of
    /// selected subprocesses) into a standalone document — the clipboard
    /// payload is plain BPMN XML.
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

        // Anything nested inside a kept subprocess travels with its
        // container's clone; only the roots are copied top-level.
        let mut nested: HashSet<BpmnId> = HashSet::new();
        for id in &keep {
            nested.extend(self.descendants_of(id));
        }

        let mut elements = Vec::new();
        for element in self.flow_elements() {
            let Some(id) = element.id() else { continue };
            if !keep.contains(id) || nested.contains(id) {
                continue;
            }
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
        if let Some(process) = out.first_process_mut() {
            process.flow_elements = elements;
        }

        let mut di_keep = keep;
        di_keep.extend(nested);
        let mut di = Vec::new();
        if let Some(plane) = self.diagram().and_then(|d| d.plane.as_ref()) {
            for element in &plane.elements {
                match element {
                    DiElement::Shape(shape) if di_keep.contains(&shape.bpmn_element) => {
                        di.push(DiElement::Shape(shape.clone()));
                    }
                    DiElement::Edge(edge) if di_keep.contains(&edge.bpmn_element) => {
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
    /// fresh ids, offset by `(dx, dy)`. Returns the new top-level node ids
    /// (not flows, not subprocess contents).
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

        // First pass: a new id for every modeled element, nested ones too.
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

        let mut new_nodes = Vec::new();
        let mut elements = Vec::new();
        // Only top-level elements paste directly; subprocess contents travel
        // inside their container's clone (remapped recursively).
        for process in source.processes() {
            for element in &process.flow_elements {
                let Some(old) = element.id() else { continue };
                if let FlowElement::SequenceFlow(flow) = element {
                    // A flow whose endpoints were not pasted makes no sense.
                    if !mapping.contains_key(&flow.source) || !mapping.contains_key(&flow.target) {
                        continue;
                    }
                }
                let Some(new) = mapping.get(old) else {
                    continue;
                };
                let mut element = element.clone();
                remap_element(&mut element, &mapping);
                if !matches!(element, FlowElement::SequenceFlow(_)) {
                    new_nodes.push(new.clone());
                }
                elements.push(element);
            }
        }
        if let Some(process) = self.first_process_mut() {
            process.flow_elements.extend(elements);
        }

        let remap = |id: &BpmnId, mapping: &HashMap<BpmnId, BpmnId>| {
            mapping.get(id).cloned().unwrap_or_else(|| id.clone())
        };
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

/// Removes doomed elements from a flow-element collection (descending into
/// subprocesses) and strips dangling references from the survivors.
fn prune_deleted(elements: &mut Vec<FlowElement>, doomed: &HashSet<BpmnId>) {
    elements.retain(|element| element.id().is_none_or(|id| !doomed.contains(id)));
    for element in elements {
        if let Some(node) = flow_node_mut(element) {
            node.incoming.retain(|id| !doomed.contains(id));
            node.outgoing.retain(|id| !doomed.contains(id));
        }
        match element {
            FlowElement::Gateway(gateway)
                if gateway
                    .default_flow
                    .as_ref()
                    .is_some_and(|id| doomed.contains(id)) =>
            {
                gateway.default_flow = None;
            }
            FlowElement::Activity(activity) => {
                if let ActivityKind::SubProcess(sub) = &mut activity.kind {
                    prune_deleted(&mut sub.flow_elements, doomed);
                }
            }
            _ => {}
        }
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

/// Rewrites every id a pasted element carries through `mapping`, descending
/// into subprocess contents. References to unpasted elements are dropped from
/// `incoming`/`outgoing` and gateway defaults, but a boundary's `attachedToRef`
/// is kept as written (validation will flag it when it dangles).
fn remap_element(element: &mut FlowElement, mapping: &std::collections::HashMap<BpmnId, BpmnId>) {
    let remap = |id: &mut BpmnId| {
        if let Some(new) = mapping.get(id) {
            *id = new.clone();
        }
    };

    match element {
        FlowElement::SequenceFlow(flow) => {
            remap(&mut flow.base.id);
            remap(&mut flow.source);
            remap(&mut flow.target);
        }
        FlowElement::Unknown(_) => {}
        other => {
            if let Some(node) = flow_node_mut(other) {
                remap(&mut node.base.id);
                for refs in [&mut node.incoming, &mut node.outgoing] {
                    refs.retain(|id| mapping.contains_key(id));
                    for id in refs {
                        remap(id);
                    }
                }
            }
            match other {
                FlowElement::Gateway(gateway) => {
                    gateway.default_flow = gateway
                        .default_flow
                        .take()
                        .filter(|id| mapping.contains_key(id))
                        .map(|mut id| {
                            remap(&mut id);
                            id
                        });
                }
                FlowElement::Event(event) => {
                    if let EventPosition::Boundary { attached_to, .. } = &mut event.position {
                        remap(attached_to);
                    }
                }
                FlowElement::Activity(activity) => {
                    if let ActivityKind::SubProcess(sub) = &mut activity.kind {
                        for child in &mut sub.flow_elements {
                            remap_element(child, mapping);
                        }
                    }
                }
                _ => {}
            }
        }
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
    fn subprocess_containment_and_cascade() {
        use crate::model::SubProcessKind;

        let mut doc = Document::new();
        let sub = doc
            .create_node(
                NodeKind::SubProcess(SubProcessKind::Embedded),
                Point::new(0.0, 0.0),
            )
            .unwrap();
        // Placed inside the expanded subprocess area → nested.
        let a = doc
            .create_node(NodeKind::StartEvent, Point::new(-100.0, 0.0))
            .unwrap();
        let b = doc
            .create_node(NodeKind::Task(TaskType::User), Point::new(50.0, 0.0))
            .unwrap();
        // Placed outside → top level.
        let outside = doc
            .create_node(NodeKind::EndEvent, Point::new(400.0, 0.0))
            .unwrap();

        assert_eq!(doc.container_of(&a), Some(sub.clone()));
        assert_eq!(doc.container_of(&b), Some(sub.clone()));
        assert_eq!(doc.container_of(&outside), Some(BpmnId::from("Process_1")));

        // Same-container connections work; cross-container ones refuse.
        let inner_flow = doc.connect(&a, &b).unwrap();
        assert_eq!(doc.container_of(&inner_flow), Some(sub.clone()));
        assert!(doc.connect(&b, &outside).is_none(), "cross-container");

        // Moving the subprocess carries its children and internal edges.
        let before = doc.node_outline(&a).unwrap().1;
        let moved: HashSet<BpmnId> = [sub.clone()].into_iter().collect();
        doc.move_shapes(&moved, 30.0, 40.0);
        let after = doc.node_outline(&a).unwrap().1;
        assert_eq!((after.x - before.x, after.y - before.y), (30.0, 40.0));

        // Deleting the subprocess takes the contents and their DI with it.
        doc.delete_elements(&moved);
        assert_eq!(doc.flow_elements().count(), 1, "only the outside node left");
        let plane = doc.diagram().unwrap().plane.as_ref().unwrap();
        assert_eq!(plane.elements.len(), 1);
    }

    #[test]
    fn collapse_hides_and_expand_refits() {
        use crate::model::SubProcessKind;

        let mut doc = Document::new();
        let sub = doc
            .create_node(
                NodeKind::SubProcess(SubProcessKind::Embedded),
                Point::new(0.0, 0.0),
            )
            .unwrap();
        let child = doc
            .create_node(NodeKind::Task(TaskType::Generic), Point::new(0.0, 0.0))
            .unwrap();

        assert!(doc.set_subprocess_expanded(&sub, false));
        let (_, bounds) = doc.node_outline(&sub).unwrap();
        assert_eq!((bounds.width, bounds.height), (100.0, 80.0));
        assert!(doc.hidden_by_collapse().contains(&child));

        assert!(doc.set_subprocess_expanded(&sub, true));
        let (_, bounds) = doc.node_outline(&sub).unwrap();
        assert!(doc.hidden_by_collapse().is_empty());
        // Re-expanded around the child (100x80 plus 40 padding each side).
        assert_eq!((bounds.width, bounds.height), (180.0, 160.0));

        // Not a subprocess → refused.
        assert!(!doc.set_subprocess_expanded(&child, false));
    }

    #[test]
    fn dragging_a_node_into_a_subprocess_reparents_it() {
        use crate::model::SubProcessKind;

        let mut doc = Document::new();
        let sub = doc
            .create_node(
                NodeKind::SubProcess(SubProcessKind::Embedded),
                Point::new(0.0, 0.0),
            )
            .unwrap();
        let task = doc
            .create_node(NodeKind::Task(TaskType::User), Point::new(500.0, 0.0))
            .unwrap();
        assert_eq!(doc.container_of(&task), Some(BpmnId::from("Process_1")));

        // Drag the task onto the subprocess body...
        let moved: HashSet<BpmnId> = [task.clone()].into_iter().collect();
        doc.move_shapes(&moved, -500.0, 0.0);
        assert_eq!(doc.container_of(&task), Some(sub.clone()));

        // ...and back out again.
        doc.move_shapes(&moved, 500.0, 0.0);
        assert_eq!(doc.container_of(&task), Some(BpmnId::from("Process_1")));
    }

    #[test]
    fn copying_a_subprocess_remints_nested_ids() {
        use crate::model::SubProcessKind;

        let mut doc = Document::new();
        let sub = doc
            .create_node(
                NodeKind::SubProcess(SubProcessKind::Embedded),
                Point::new(0.0, 0.0),
            )
            .unwrap();
        let a = doc
            .create_node(NodeKind::StartEvent, Point::new(-100.0, 0.0))
            .unwrap();
        let b = doc
            .create_node(NodeKind::EndEvent, Point::new(100.0, 0.0))
            .unwrap();
        doc.connect(&a, &b).unwrap();

        let selection: HashSet<BpmnId> = [sub.clone()].into_iter().collect();
        let clipboard = doc.extract(&selection);
        // The clone carries the nested elements and their DI.
        assert_eq!(clipboard.flow_elements().count(), 4);
        assert_eq!(
            clipboard
                .diagram()
                .unwrap()
                .plane
                .as_ref()
                .unwrap()
                .elements
                .len(),
            4
        );

        let pasted = doc.paste_from(&clipboard, 500.0, 0.0);
        assert_eq!(pasted.len(), 1, "one top-level node pasted");
        assert_eq!(doc.flow_elements().count(), 8);

        // Nested ids were reminted: the pasted subprocess's flow connects the
        // pasted children, not the originals.
        let index = doc.flow_element_index();
        let FlowElement::Activity(activity) = index[&pasted[0]] else {
            panic!()
        };
        let ActivityKind::SubProcess(sub) = &activity.kind else {
            panic!()
        };
        let FlowElement::SequenceFlow(flow) = sub
            .flow_elements
            .iter()
            .find(|e| matches!(e, FlowElement::SequenceFlow(_)))
            .unwrap()
        else {
            panic!()
        };
        assert_ne!(flow.source, a);
        assert_eq!(doc.container_of(&flow.source).as_ref(), Some(&pasted[0]));
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
