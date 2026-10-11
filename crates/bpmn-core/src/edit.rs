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
    Activity, ActivityKind, Artifact, ArtifactKind, BOSS_TARGET_NAMESPACE, BaseElement,
    CallActivity, Collaboration, Condition, DataElement, DataKind, Definitions, Event,
    EventPosition, FlowElement, FlowNode, Gateway, GatewayKind, Lane, LaneSet, LoopKind,
    LoopMarker, MessageFlow, Participant, Process, RootElement, SequenceFlow, SubProcess,
    SubProcessKind, TaskType,
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
    /// A data object reference (also creates its backing `dataObject`).
    DataObject,
    /// A data store reference.
    DataStore,
    /// A text annotation.
    TextAnnotation,
    /// A group.
    Group,
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
            NodeKind::Gateway(_) | NodeKind::DataStore => Size::new(50.0, 50.0),
            NodeKind::SubProcess(_) => Size::new(350.0, 200.0),
            NodeKind::DataObject => Size::new(36.0, 50.0),
            NodeKind::TextAnnotation => Size::new(120.0, 40.0),
            NodeKind::Group => Size::new(300.0, 200.0),
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
            NodeKind::DataObject => "DataObjectReference",
            NodeKind::DataStore => "DataStoreReference",
            NodeKind::TextAnnotation => "TextAnnotation",
            NodeKind::Group => "Group",
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

/// The outline of a modeled flow node, data element, or boxy artifact.
pub fn outline_of(element: &FlowElement) -> Option<Outline> {
    match element {
        FlowElement::Event(_) => Some(Outline::Circle),
        FlowElement::Activity(_) | FlowElement::Data(_) => Some(Outline::Rect),
        FlowElement::Gateway(_) => Some(Outline::Diamond),
        FlowElement::Artifact(artifact) => match artifact.kind {
            ArtifactKind::TextAnnotation { .. } | ArtifactKind::Group { .. } => Some(Outline::Rect),
            ArtifactKind::Association { .. } => None,
        },
        FlowElement::SequenceFlow(_) | FlowElement::Unknown(_) => None,
    }
}

impl Document {
    /// A fresh, empty document: one executable-less process and its plane.
    pub fn new() -> Self {
        let process_id = BpmnId::new("Process_1");
        let process = Process {
            base: BaseElement::new(process_id.clone()),
            is_executable: Some(false),
            ..Process::default()
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
        fn push_lanes(sets: &[LaneSet], push: &mut impl FnMut(Option<&BpmnId>)) {
            for set in sets {
                push(set.id.as_ref());
                for lane in &set.lanes {
                    push(Some(&lane.base.id));
                    if let Some(child) = &lane.child_lane_set {
                        push_lanes(std::slice::from_ref(child), push);
                    }
                }
            }
        }

        let mut ids = HashSet::new();
        let mut push = |id: Option<&BpmnId>| {
            if let Some(id) = id {
                ids.insert(id.as_str().to_string());
            }
        };

        push(self.definitions.id.as_ref());
        for root in &self.definitions.root_elements {
            match root {
                RootElement::Process(process) => {
                    push(Some(&process.base.id));
                    push_lanes(&process.lane_sets, &mut push);
                }
                RootElement::Collaboration(collaboration) => {
                    push(Some(&collaboration.base.id));
                    for participant in &collaboration.participants {
                        push(Some(&participant.base.id));
                    }
                    for flow in &collaboration.message_flows {
                        push(Some(&flow.base.id));
                    }
                }
                RootElement::Unknown(_) => {}
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
                _ => None,
            })
    }

    fn collaboration_mut(&mut self) -> Option<&mut Collaboration> {
        self.definitions
            .root_elements
            .iter_mut()
            .find_map(|root| match root {
                RootElement::Collaboration(collaboration) => Some(collaboration),
                _ => None,
            })
    }

    fn participant_mut(&mut self, id: &BpmnId) -> Option<&mut Participant> {
        self.collaboration_mut()?
            .participants
            .iter_mut()
            .find(|p| p.base.id == *id)
    }

    fn message_flow_mut(&mut self, id: &BpmnId) -> Option<&mut MessageFlow> {
        self.collaboration_mut()?
            .message_flows
            .iter_mut()
            .find(|m| m.base.id == *id)
    }

    fn lane_mut(&mut self, id: &BpmnId) -> Option<&mut Lane> {
        fn scan<'a>(sets: &'a mut [LaneSet], id: &BpmnId) -> Option<&'a mut Lane> {
            for set in sets {
                for lane in &mut set.lanes {
                    if lane.base.id == *id {
                        return Some(lane);
                    }
                    if let Some(child) = &mut lane.child_lane_set
                        && let Some(found) = scan(std::slice::from_mut(child), id)
                    {
                        return Some(found);
                    }
                }
            }
            None
        }
        self.definitions
            .root_elements
            .iter_mut()
            .filter_map(|root| match root {
                RootElement::Process(process) => Some(process),
                _ => None,
            })
            .find_map(|process| scan(&mut process.lane_sets, id))
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

    /// The deepest container whose shape contains `point`: an expanded
    /// subprocess, or the process behind a pool. Without a collaboration the
    /// first process is the fallback canvas; with one, a point outside every
    /// pool belongs nowhere (`None`). `except` (a node plus its descendants)
    /// never counts as a container, so a subprocess cannot be dropped into
    /// itself.
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
            if except.contains(id) || hidden.contains(id) || !shape.bounds.contains(point) {
                continue;
            }
            // A container resolves to the collection that adopts the node:
            // the subprocess itself, or a pool's backing process.
            let container = match index.get(id).copied() {
                Some(FlowElement::Activity(activity))
                    if matches!(activity.kind, ActivityKind::SubProcess(_))
                        && shape.is_expanded == Some(true) =>
                {
                    id.clone()
                }
                _ => match self.participant(id).and_then(|p| p.process_ref.clone()) {
                    Some(process) => process,
                    None => continue,
                },
            };
            let area = shape.bounds.width * shape.bounds.height;
            if best.as_ref().is_none_or(|(_, a)| area < *a) {
                best = Some((container, area));
            }
        }

        best.map(|(id, _)| id).or_else(|| {
            // In a collaboration everything lives in some pool.
            (self.collaboration().is_none())
                .then(|| self.processes().next().map(|p| p.base.id.clone()))
                .flatten()
        })
    }

    /// Builds the semantic element for a palette kind, plus an optional
    /// backing element (a `dataObject` behind a new object reference).
    /// Boundary events return `None`: they need a host.
    fn build_node_element(
        &self,
        kind: NodeKind,
        id: &BpmnId,
    ) -> Option<(FlowElement, Option<FlowElement>)> {
        let node = FlowNode {
            base: BaseElement::new(id.clone()),
            ..FlowNode::default()
        };
        let mut backing = None;
        let element = match kind {
            NodeKind::DataObject => {
                let object_id = self.fresh_id("DataObject");
                backing = Some(FlowElement::Data(DataElement {
                    base: BaseElement::new(object_id.clone()),
                    name: None,
                    kind: DataKind::Object,
                }));
                FlowElement::Data(DataElement {
                    base: BaseElement::new(id.clone()),
                    name: None,
                    kind: DataKind::ObjectReference {
                        data_object_ref: Some(object_id),
                    },
                })
            }
            NodeKind::DataStore => FlowElement::Data(DataElement {
                base: BaseElement::new(id.clone()),
                name: None,
                kind: DataKind::StoreReference {
                    data_store_ref: None,
                },
            }),
            NodeKind::TextAnnotation => FlowElement::Artifact(Artifact {
                base: BaseElement::new(id.clone()),
                kind: ArtifactKind::TextAnnotation { text: None },
            }),
            NodeKind::Group => FlowElement::Artifact(Artifact {
                base: BaseElement::new(id.clone()),
                kind: ArtifactKind::Group {
                    category_value_ref: None,
                },
            }),
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
        Some((element, backing))
    }

    /// Creates a flow node of `kind` centered at `center`; returns its id.
    /// The node lands in the expanded subprocess under `center`, if any.
    pub fn create_node(&mut self, kind: NodeKind, center: Point) -> Option<BpmnId> {
        let id = self.fresh_id(kind.id_prefix());
        let shape_id = BpmnId::new(format!("{id}_di").as_str());
        let size = kind.default_size();
        let container = self.container_at(center, &HashSet::new())?;

        let (element, backing) = self.build_node_element(kind, &id)?;

        let elements = self.elements_of_mut(&container)?;
        if let Some(backing) = backing {
            elements.push(backing);
        }
        elements.push(element);
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
        self.assign_lanes();

        Some(id)
    }

    /// Connects two elements: nodes in the same container get a sequence
    /// flow; endpoints in different pools (nodes or the pools themselves) get
    /// a message flow; anything touching an annotation, group, or data
    /// element gets a dotted association. Refuses self-connections, unknown
    /// endpoints, and nodes in different containers of the same process.
    pub fn connect(&mut self, source: &BpmnId, target: &BpmnId) -> Option<BpmnId> {
        if source == target {
            return None;
        }
        let artifact_end = |id: &BpmnId| {
            matches!(
                self.flow_element_index().get(id),
                Some(
                    FlowElement::Data(_)
                        | FlowElement::Artifact(Artifact {
                            kind: ArtifactKind::TextAnnotation { .. } | ArtifactKind::Group { .. },
                            ..
                        })
                )
            )
        };
        if artifact_end(source) || artifact_end(target) {
            return self.connect_association(source, target);
        }
        match (self.container_of(source), self.container_of(target)) {
            (Some(sc), Some(tc)) if sc == tc => self.connect_sequence(source, target, &sc),
            _ => self.connect_message(source, target),
        }
    }

    /// Creates an `association` between two elements; it lives in the
    /// source's container (or the target's, when the source sits elsewhere).
    fn connect_association(&mut self, source: &BpmnId, target: &BpmnId) -> Option<BpmnId> {
        let container = self
            .container_of(source)
            .or_else(|| self.container_of(target))?;
        let (source_outline, source_bounds) = self.node_outline(source)?;
        let (target_outline, target_bounds) = self.node_outline(target)?;

        let id = self.fresh_id("Association");
        let edge_id = BpmnId::new(format!("{id}_di").as_str());
        let from = source_outline.anchor(source_bounds, target_bounds.center());
        let to = target_outline.anchor(target_bounds, source_bounds.center());

        self.elements_of_mut(&container)?
            .push(FlowElement::Artifact(Artifact {
                base: BaseElement::new(id.clone()),
                kind: ArtifactKind::Association {
                    source: source.clone(),
                    target: target.clone(),
                    direction: None,
                },
            }));
        self.plane_mut()?.elements.push(DiElement::Edge(BpmnEdge {
            id: Some(edge_id),
            bpmn_element: id.clone(),
            waypoints: vec![from, to],
            ..BpmnEdge::default()
        }));
        Some(id)
    }

    fn connect_sequence(
        &mut self,
        source: &BpmnId,
        target: &BpmnId,
        container: &BpmnId,
    ) -> Option<BpmnId> {
        let (source_outline, source_bounds) = self.node_outline(source)?;
        let (target_outline, target_bounds) = self.node_outline(target)?;

        let id = self.fresh_id("Flow");
        let edge_id = BpmnId::new(format!("{id}_di").as_str());

        let from = source_outline.anchor(source_bounds, target_bounds.center());
        let to = target_outline.anchor(target_bounds, source_bounds.center());

        self.elements_of_mut(container)?
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

    /// The pool identity of a message-flow endpoint: the backing process for
    /// nodes and process-backed participants, the participant itself for
    /// black-box pools. Endpoints with the same key cannot exchange messages.
    fn endpoint_pool(&self, id: &BpmnId) -> Option<BpmnId> {
        if let Some(participant) = self.participant(id) {
            return Some(
                participant
                    .process_ref
                    .clone()
                    .unwrap_or_else(|| participant.base.id.clone()),
            );
        }
        self.process_of(id)
    }

    fn connect_message(&mut self, source: &BpmnId, target: &BpmnId) -> Option<BpmnId> {
        // Message flows need a collaboration and endpoints in different pools.
        self.collaboration()?;
        if self.endpoint_pool(source)? == self.endpoint_pool(target)? {
            return None;
        }
        let (source_outline, source_bounds) = self.node_outline(source)?;
        let (target_outline, target_bounds) = self.node_outline(target)?;

        let id = self.fresh_id("MessageFlow");
        let edge_id = BpmnId::new(format!("{id}_di").as_str());
        let from = source_outline.anchor(source_bounds, target_bounds.center());
        let to = target_outline.anchor(target_bounds, source_bounds.center());

        self.collaboration_mut()?.message_flows.push(MessageFlow {
            base: BaseElement::new(id.clone()),
            name: None,
            source: source.clone(),
            target: target.clone(),
        });
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

    /// The outline and DI bounds of a flow node or pool.
    pub fn node_outline(&self, id: &BpmnId) -> Option<(Outline, Bounds)> {
        let index = self.flow_element_index();
        let outline = match index.get(id).copied() {
            Some(element) => outline_of(element)?,
            // Pools anchor message flows on their rectangle.
            None => self.participant(id).map(|_| Outline::Rect)?,
        };
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
            moving.extend(self.pool_members(id));
        }
        moving.extend(self.attached_boundaries(&moving));

        // Edges fully inside the moving set travel rigidly (all waypoints);
        // edges with one end inside get their endpoints re-anchored. Message
        // flows behave the same, with pools counting as endpoints.
        let mut rigid: HashSet<BpmnId> = HashSet::new();
        let mut touched: Vec<(BpmnId, BpmnId, BpmnId)> = Vec::new();
        let flow_ends = self
            .flow_elements()
            .filter_map(|element| match element {
                FlowElement::SequenceFlow(flow) => {
                    Some((&flow.base.id, &flow.source, &flow.target))
                }
                FlowElement::Artifact(Artifact {
                    base,
                    kind: ArtifactKind::Association { source, target, .. },
                }) => Some((&base.id, source, target)),
                _ => None,
            })
            .chain(
                self.message_flows()
                    .map(|flow| (&flow.base.id, &flow.source, &flow.target)),
            );
        for (id, source, target) in flow_ends {
            match (moving.contains(source), moving.contains(target)) {
                (true, true) => {
                    rigid.insert(id.clone());
                }
                (true, false) | (false, true) => {
                    touched.push((id.clone(), source.clone(), target.clone()));
                }
                (false, false) => {}
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
        self.assign_lanes();
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
                Some(FlowElement::Activity(_) | FlowElement::Gateway(_) | FlowElement::Data(_)) => {
                    true
                }
                Some(FlowElement::Artifact(artifact)) => {
                    !matches!(artifact.kind, ArtifactKind::Association { .. })
                }
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

    /// Copies the DI of every mapped element from `source`, reminted and
    /// offset by `(dx, dy)`.
    fn paste_di(
        &mut self,
        source: &Document,
        mapping: &std::collections::HashMap<BpmnId, BpmnId>,
        dx: f64,
        dy: f64,
    ) {
        let remap = |id: &BpmnId| mapping.get(id).cloned().unwrap_or_else(|| id.clone());
        let mut di = Vec::new();
        if let Some(plane) = source.diagram().and_then(|d| d.plane.as_ref()) {
            for element in &plane.elements {
                match element {
                    DiElement::Shape(shape) if mapping.contains_key(&shape.bpmn_element) => {
                        let mut shape = shape.clone();
                        shape.bpmn_element = remap(&shape.bpmn_element);
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
                        edge.bpmn_element = remap(&edge.bpmn_element);
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

    /// Creates a pool (participant) centered at `at`; returns its id. The
    /// first pool adopts the existing default process (wrapping its content);
    /// later pools each get a fresh, empty process. Creating the first pool
    /// also creates the collaboration and points the plane at it.
    pub fn create_pool(&mut self, name: Option<String>, at: Point) -> Option<BpmnId> {
        const POOL_HEADER: f64 = 30.0;
        const PADDING: f64 = 40.0;
        const DEFAULT_SIZE: Size = Size {
            width: 600.0,
            height: 250.0,
        };

        let id = self.fresh_id("Participant");
        let shape_id = BpmnId::new(format!("{id}_di").as_str());

        if self.collaboration().is_none() {
            let collaboration_id = self.fresh_id("Collaboration");
            self.definitions.root_elements.insert(
                0,
                RootElement::Collaboration(Collaboration {
                    base: BaseElement::new(collaboration_id.clone()),
                    ..Collaboration::default()
                }),
            );
            if let Some(plane) = self.plane_mut() {
                plane.bpmn_element = Some(collaboration_id);
            }
        }

        // Adopt the first process no pool references yet, else make a new one.
        let referenced: HashSet<BpmnId> = self
            .collaboration()
            .into_iter()
            .flat_map(|c| &c.participants)
            .filter_map(|p| p.process_ref.clone())
            .collect();
        let adopted = self
            .processes()
            .map(|p| p.base.id.clone())
            .find(|id| !referenced.contains(id));
        let process_ref = adopted.clone().unwrap_or_else(|| {
            let process_id = self.fresh_id("Process");
            self.definitions
                .root_elements
                .push(RootElement::Process(Process {
                    base: BaseElement::new(process_id.clone()),
                    is_executable: Some(false),
                    ..Process::default()
                }));
            process_id
        });

        // Wrap existing content when adopting a non-empty process; otherwise
        // place an empty pool at the requested point.
        let content = adopted.and_then(|process_id| {
            let members: HashSet<BpmnId> = self.pool_ids_of_process(&process_id);
            let plane = self.diagram()?.plane.as_ref()?;
            let mut bbox: Option<Bounds> = None;
            for element in &plane.elements {
                if let DiElement::Shape(shape) = element
                    && members.contains(&shape.bpmn_element)
                {
                    bbox = Some(bbox.map_or(shape.bounds, |b| b.union(shape.bounds)));
                }
            }
            bbox
        });
        let bounds = content.map_or_else(
            || {
                Bounds::new(
                    at.x - DEFAULT_SIZE.width / 2.0,
                    at.y - DEFAULT_SIZE.height / 2.0,
                    DEFAULT_SIZE.width,
                    DEFAULT_SIZE.height,
                )
            },
            |b| {
                Bounds::new(
                    b.x - PADDING - POOL_HEADER,
                    b.y - PADDING,
                    b.width + PADDING * 2.0 + POOL_HEADER,
                    b.height + PADDING * 2.0,
                )
            },
        );

        self.collaboration_mut()?.participants.push(Participant {
            base: BaseElement::new(id.clone()),
            name,
            process_ref: Some(process_ref),
        });
        self.plane_mut()?.elements.push(DiElement::Shape(BpmnShape {
            id: Some(shape_id),
            bpmn_element: id.clone(),
            bounds,
            is_horizontal: Some(true),
            ..BpmnShape::default()
        }));
        Some(id)
    }

    /// Appends a lane to a pool; returns the lane's id. The first lane covers
    /// the pool's body; each further lane grows the pool downward.
    pub fn add_lane(&mut self, pool: &BpmnId, name: Option<String>) -> Option<BpmnId> {
        const POOL_HEADER: f64 = 30.0;
        const LANE_HEIGHT: f64 = 120.0;

        let process_id = self.participant(pool)?.process_ref.clone()?;
        let (_, pool_bounds) = self.node_outline(pool)?;
        let existing = self
            .processes()
            .find(|p| p.base.id == process_id)
            .map_or(0, |p| p.lane_sets.iter().map(|s| s.lanes.len()).sum());

        let id = self.fresh_id("Lane");
        let shape_id = BpmnId::new(format!("{id}_di").as_str());
        let bounds = if existing == 0 {
            Bounds::new(
                pool_bounds.x + POOL_HEADER,
                pool_bounds.y,
                pool_bounds.width - POOL_HEADER,
                pool_bounds.height,
            )
        } else {
            Bounds::new(
                pool_bounds.x + POOL_HEADER,
                pool_bounds.y + pool_bounds.height,
                pool_bounds.width - POOL_HEADER,
                LANE_HEIGHT,
            )
        };

        let lane_set_id = self.fresh_id("LaneSet");
        let process = self
            .definitions
            .root_elements
            .iter_mut()
            .find_map(|root| match root {
                RootElement::Process(process) if process.base.id == process_id => Some(process),
                _ => None,
            })?;
        if process.lane_sets.is_empty() {
            process.lane_sets.push(LaneSet {
                id: Some(lane_set_id),
                ..LaneSet::default()
            });
        }
        process.lane_sets[0].lanes.push(Lane {
            base: BaseElement::new(id.clone()),
            name,
            ..Lane::default()
        });

        if existing > 0 {
            // Grow the pool to make room for the new lane.
            if let Some(plane) = self.plane_mut() {
                for element in &mut plane.elements {
                    if let DiElement::Shape(shape) = element
                        && shape.bpmn_element == *pool
                    {
                        shape.bounds.height += LANE_HEIGHT;
                    }
                }
            }
        }
        self.plane_mut()?.elements.push(DiElement::Shape(BpmnShape {
            id: Some(shape_id),
            bpmn_element: id.clone(),
            bounds,
            is_horizontal: Some(true),
            ..BpmnShape::default()
        }));
        self.assign_lanes();
        Some(id)
    }

    /// Every element id belonging to the pool `id` (its process's contents,
    /// at any depth, plus its lanes). Empty when `id` is not a pool.
    fn pool_members(&self, id: &BpmnId) -> Vec<BpmnId> {
        let Some(process_id) = self.participant(id).and_then(|p| p.process_ref.clone()) else {
            return Vec::new();
        };
        self.pool_ids_of_process(&process_id).into_iter().collect()
    }

    /// The element and lane ids of one process (contents at any depth).
    fn pool_ids_of_process(&self, process_id: &BpmnId) -> HashSet<BpmnId> {
        fn collect(elements: &[FlowElement], out: &mut HashSet<BpmnId>) {
            for element in elements {
                if let Some(id) = element.id() {
                    out.insert(id.clone());
                }
                if let FlowElement::Activity(activity) = element
                    && let ActivityKind::SubProcess(sub) = &activity.kind
                {
                    collect(&sub.flow_elements, out);
                }
            }
        }
        fn collect_lanes(sets: &[LaneSet], out: &mut HashSet<BpmnId>) {
            for set in sets {
                for lane in &set.lanes {
                    out.insert(lane.base.id.clone());
                    if let Some(child) = &lane.child_lane_set {
                        collect_lanes(std::slice::from_ref(child), out);
                    }
                }
            }
        }
        let mut out = HashSet::new();
        if let Some(process) = self.processes().find(|p| p.base.id == *process_id) {
            collect(&process.flow_elements, &mut out);
            collect_lanes(&process.lane_sets, &mut out);
        }
        out
    }

    /// Rebuilds `flowNodeRef` lists from geometry: each top-level node of a
    /// process belongs to the smallest DI-backed lane containing its center.
    /// Lanes without DI bounds keep their references as loaded.
    fn assign_lanes(&mut self) {
        use std::collections::HashMap;

        // Writes the refs back into every DI-backed lane.
        fn apply(
            sets: &mut [LaneSet],
            assignments: &HashMap<BpmnId, Vec<BpmnId>>,
            backed: &HashMap<BpmnId, Bounds>,
        ) {
            for set in sets {
                for lane in &mut set.lanes {
                    if backed.contains_key(&lane.base.id) {
                        lane.flow_node_refs =
                            assignments.get(&lane.base.id).cloned().unwrap_or_default();
                    }
                    if let Some(child) = &mut lane.child_lane_set {
                        apply(std::slice::from_mut(child), assignments, backed);
                    }
                }
            }
        }

        // Lane bounds from DI.
        let mut lane_bounds: HashMap<BpmnId, Bounds> = HashMap::new();
        if let Some(plane) = self.diagram().and_then(|d| d.plane.as_ref()) {
            for element in &plane.elements {
                if let DiElement::Shape(shape) = element
                    && self.lane(&shape.bpmn_element).is_some()
                {
                    lane_bounds.insert(shape.bpmn_element.clone(), shape.bounds);
                }
            }
        }
        if lane_bounds.is_empty() {
            return;
        }

        // For each process: node centers → smallest containing lane.
        let mut assignments: HashMap<BpmnId, Vec<BpmnId>> = HashMap::new();
        let process_ids: Vec<BpmnId> = self.processes().map(|p| p.base.id.clone()).collect();
        for process_id in &process_ids {
            let lane_ids: Vec<BpmnId> = self
                .pool_ids_of_process(process_id)
                .into_iter()
                .filter(|id| lane_bounds.contains_key(id))
                .collect();
            if lane_ids.is_empty() {
                continue;
            }
            let process = self.processes().find(|p| p.base.id == *process_id);
            let Some(process) = process else { continue };
            for element in &process.flow_elements {
                let is_node = matches!(
                    element,
                    FlowElement::Event(_) | FlowElement::Activity(_) | FlowElement::Gateway(_)
                );
                let Some(id) = element.id().filter(|_| is_node) else {
                    continue;
                };
                let Some((_, bounds)) = self.node_outline(id) else {
                    continue;
                };
                let center = bounds.center();
                let mut best: Option<(&BpmnId, f64)> = None;
                for lane_id in &lane_ids {
                    let lb = lane_bounds[lane_id];
                    if lb.contains(center) {
                        let area = lb.width * lb.height;
                        if best.is_none_or(|(_, a)| area < a) {
                            best = Some((lane_id, area));
                        }
                    }
                }
                if let Some((lane_id, _)) = best {
                    assignments
                        .entry(lane_id.clone())
                        .or_default()
                        .push(id.clone());
                }
            }
        }

        for root in &mut self.definitions.root_elements {
            if let RootElement::Process(process) = root {
                apply(&mut process.lane_sets, &assignments, &lane_bounds);
            }
        }
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

    /// Deletes elements (plus the contents of deleted subprocesses and pools,
    /// and every flow touching a deleted element) and their DI, and strips
    /// dangling references from the survivors. Deleting the last pool
    /// dissolves the collaboration back into a plain process diagram.
    pub fn delete_elements(&mut self, ids: &HashSet<BpmnId>) {
        // Expand the set with subprocess and pool contents, then with flows
        // that touch deleted elements.
        let mut doomed: HashSet<BpmnId> = ids.clone();
        for id in ids {
            doomed.extend(self.descendants_of(id));
            doomed.extend(self.pool_members(id));
        }
        for element in self.flow_elements() {
            match element {
                FlowElement::SequenceFlow(flow)
                    if doomed.contains(&flow.source) || doomed.contains(&flow.target) =>
                {
                    doomed.insert(flow.base.id.clone());
                }
                FlowElement::Artifact(Artifact {
                    base,
                    kind: ArtifactKind::Association { source, target, .. },
                }) if doomed.contains(source) || doomed.contains(target) => {
                    doomed.insert(base.id.clone());
                }
                _ => {}
            }
        }
        let message_doomed: Vec<BpmnId> = self
            .message_flows()
            .filter(|flow| {
                doomed.contains(&flow.base.id)
                    || doomed.contains(&flow.source)
                    || doomed.contains(&flow.target)
            })
            .map(|flow| flow.base.id.clone())
            .collect();
        doomed.extend(message_doomed);

        // Processes backing deleted pools die with them.
        let dead_processes: HashSet<BpmnId> = self
            .collaboration()
            .into_iter()
            .flat_map(|c| &c.participants)
            .filter(|p| doomed.contains(&p.base.id))
            .filter_map(|p| p.process_ref.clone())
            .collect();

        if let Some(collaboration) = self.collaboration_mut() {
            collaboration
                .participants
                .retain(|p| !doomed.contains(&p.base.id));
            collaboration
                .message_flows
                .retain(|m| !doomed.contains(&m.base.id));
        }
        self.definitions.root_elements.retain(|root| match root {
            RootElement::Process(process) => !dead_processes.contains(&process.base.id),
            _ => true,
        });

        for root in &mut self.definitions.root_elements {
            if let RootElement::Process(process) = root {
                prune_deleted(&mut process.flow_elements, &doomed);
                prune_lanes(&mut process.lane_sets, &doomed);
            }
        }

        if let Some(plane) = self.plane_mut() {
            plane.elements.retain(|element| match element {
                DiElement::Shape(shape) => !doomed.contains(&shape.bpmn_element),
                DiElement::Edge(edge) => !doomed.contains(&edge.bpmn_element),
                DiElement::Unknown(_) => true,
            });
        }

        // A collaboration without pools dissolves; the canvas shows a plain
        // process again (a fresh one if the last pool took every process).
        if self
            .collaboration()
            .is_some_and(|c| c.participants.is_empty())
        {
            self.definitions
                .root_elements
                .retain(|root| !matches!(root, RootElement::Collaboration(_)));
            if self.processes().next().is_none() {
                let process_id = self.fresh_id("Process");
                self.definitions
                    .root_elements
                    .push(RootElement::Process(Process {
                        base: BaseElement::new(process_id),
                        is_executable: Some(false),
                        ..Process::default()
                    }));
            }
            let first = self.processes().next().map(|p| p.base.id.clone());
            if let Some(plane) = self.plane_mut() {
                plane.bpmn_element = first;
            }
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
                _ => None,
            })
            .find_map(|process| scan(&mut process.flow_elements, id))
    }

    /// Renames a flow element, pool, lane, or message flow (`None` removes
    /// the name).
    pub fn set_name(&mut self, id: &BpmnId, name: Option<String>) -> bool {
        fn set(slot: &mut Option<String>, name: Option<String>) -> bool {
            *slot = name;
            true
        }

        let name = name.filter(|n| !n.is_empty());
        match self.flow_element_mut(id) {
            Some(FlowElement::Event(e)) => return set(&mut e.node.name, name),
            Some(FlowElement::Activity(a)) => return set(&mut a.node.name, name),
            Some(FlowElement::Gateway(g)) => return set(&mut g.node.name, name),
            Some(FlowElement::SequenceFlow(f)) => return set(&mut f.name, name),
            Some(FlowElement::Data(d)) => return set(&mut d.name, name),
            // An annotation's editable "name" is its text.
            Some(FlowElement::Artifact(Artifact {
                kind: ArtifactKind::TextAnnotation { text },
                ..
            })) => return set(text, name),
            _ => {}
        }
        if let Some(participant) = self.participant_mut(id) {
            return set(&mut participant.name, name);
        }
        if let Some(lane) = self.lane_mut(id) {
            return set(&mut lane.name, name);
        }
        if let Some(flow) = self.message_flow_mut(id) {
            return set(&mut flow.name, name);
        }
        false
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
            Some(FlowElement::Artifact(Artifact {
                kind: ArtifactKind::Association { source, target, .. },
                ..
            })) => ((*source).clone(), (*target).clone()),
            _ => match self.message_flows().find(|m| m.base.id == *flow_id) {
                Some(flow) => (flow.source.clone(), flow.target.clone()),
                None => return false,
            },
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

        let mut keep: HashSet<BpmnId> = self
            .flow_elements()
            .filter_map(|element| match element {
                FlowElement::SequenceFlow(flow) => (selection.contains(&flow.source)
                    && selection.contains(&flow.target))
                .then(|| flow.base.id.clone()),
                FlowElement::Artifact(Artifact {
                    base,
                    kind: ArtifactKind::Association { source, target, .. },
                }) => (selection.contains(source) && selection.contains(target))
                    .then(|| base.id.clone()),
                other => other.id().filter(|id| selection.contains(id)).cloned(),
            })
            .collect();
        // Backing data objects travel with their kept references.
        let backing: Vec<BpmnId> = self
            .flow_elements()
            .filter_map(|element| match element {
                FlowElement::Data(DataElement {
                    base,
                    kind:
                        DataKind::ObjectReference {
                            data_object_ref: Some(object),
                        },
                    ..
                }) if keep.contains(&base.id) => Some(object.clone()),
                _ => None,
            })
            .collect();
        keep.extend(backing);

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
                FlowElement::Data(data) => match data.kind {
                    DataKind::Object => "DataObject",
                    DataKind::ObjectReference { .. } => "DataObjectReference",
                    DataKind::StoreReference { .. } => "DataStoreReference",
                },
                FlowElement::Artifact(artifact) => match artifact.kind {
                    ArtifactKind::TextAnnotation { .. } => "TextAnnotation",
                    ArtifactKind::Association { .. } => "Association",
                    ArtifactKind::Group { .. } => "Group",
                },
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
                // An edge whose endpoints were not pasted makes no sense.
                match element {
                    FlowElement::SequenceFlow(flow)
                        if !mapping.contains_key(&flow.source)
                            || !mapping.contains_key(&flow.target) =>
                    {
                        continue;
                    }
                    FlowElement::Artifact(Artifact {
                        kind: ArtifactKind::Association { source, target, .. },
                        ..
                    }) if !mapping.contains_key(source) || !mapping.contains_key(target) => {
                        continue;
                    }
                    _ => {}
                }
                let Some(new) = mapping.get(old) else {
                    continue;
                };
                let mut element = element.clone();
                remap_element(&mut element, &mapping);
                let is_edge = matches!(
                    &element,
                    FlowElement::SequenceFlow(_)
                        | FlowElement::Artifact(Artifact {
                            kind: ArtifactKind::Association { .. },
                            ..
                        })
                );
                if !is_edge {
                    new_nodes.push(new.clone());
                }
                elements.push(element);
            }
        }
        if let Some(process) = self.first_process_mut() {
            process.flow_elements.extend(elements);
        }

        self.paste_di(source, &mapping, dx, dy);

        // Pasted geometry decides containment and lane membership, exactly
        // like a drop would.
        let pasted: HashSet<BpmnId> = new_nodes.iter().cloned().collect();
        self.reparent_moved(&pasted);
        self.assign_lanes();

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

/// Removes doomed lanes (descending into child lane sets) and strips doomed
/// node references from the lanes that remain. Emptied lane sets disappear.
fn prune_lanes(sets: &mut Vec<LaneSet>, doomed: &HashSet<BpmnId>) {
    for set in sets.iter_mut() {
        set.lanes.retain(|lane| !doomed.contains(&lane.base.id));
        for lane in &mut set.lanes {
            lane.flow_node_refs.retain(|id| !doomed.contains(id));
            if let Some(child) = &mut lane.child_lane_set {
                let mut child_sets = vec![std::mem::take(child)];
                prune_lanes(&mut child_sets, doomed);
                lane.child_lane_set = child_sets.pop().filter(|s| !s.lanes.is_empty());
            }
        }
    }
    sets.retain(|set| !set.lanes.is_empty());
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
        FlowElement::SequenceFlow(_)
        | FlowElement::Data(_)
        | FlowElement::Artifact(_)
        | FlowElement::Unknown(_) => None,
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
        FlowElement::Data(data) => {
            remap(&mut data.base.id);
            match &mut data.kind {
                DataKind::ObjectReference {
                    data_object_ref: Some(object),
                } => remap(object),
                DataKind::StoreReference {
                    data_store_ref: Some(store),
                } => remap(store),
                _ => {}
            }
        }
        FlowElement::Artifact(artifact) => {
            remap(&mut artifact.base.id);
            if let ArtifactKind::Association { source, target, .. } = &mut artifact.kind {
                remap(source);
                remap(target);
            }
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
    fn pools_wrap_adopt_and_message_flows_connect_them() {
        let mut doc = Document::new();
        let start = doc
            .create_node(NodeKind::StartEvent, Point::new(0.0, 0.0))
            .unwrap();
        let task = doc
            .create_node(NodeKind::Task(TaskType::User), Point::new(200.0, 0.0))
            .unwrap();
        doc.connect(&start, &task).unwrap();

        // The first pool adopts Process_1 and wraps its content.
        let pool_a = doc
            .create_pool(Some(String::from("Shop")), Point::new(0.0, 0.0))
            .unwrap();
        assert!(doc.collaboration().is_some());
        assert_eq!(
            doc.participant(&pool_a).unwrap().process_ref,
            Some(BpmnId::from("Process_1"))
        );
        let (_, bounds_a) = doc.node_outline(&pool_a).unwrap();
        let (_, task_bounds) = doc.node_outline(&task).unwrap();
        assert!(
            bounds_a.contains(task_bounds.center()),
            "pool wraps content"
        );
        // The plane now shows the collaboration.
        let plane_ref = doc.diagram().unwrap().plane.as_ref().unwrap();
        assert_eq!(
            plane_ref.bpmn_element,
            Some(doc.collaboration().unwrap().base.id.clone())
        );

        // A second pool gets its own fresh process.
        let pool_b = doc
            .create_pool(Some(String::from("Customer")), Point::new(0.0, 500.0))
            .unwrap();
        let process_b = doc
            .participant(&pool_b)
            .unwrap()
            .process_ref
            .clone()
            .unwrap();
        assert_ne!(process_b.as_str(), "Process_1");

        // A node placed inside pool B joins process B.
        let order = doc
            .create_node(NodeKind::StartEvent, Point::new(0.0, 500.0))
            .unwrap();
        assert_eq!(doc.container_of(&order), Some(process_b));
        // A node placed outside every pool is refused.
        assert!(
            doc.create_node(NodeKind::EndEvent, Point::new(2000.0, 2000.0))
                .is_none()
        );

        // Cross-pool connect produces a message flow; same-pool a sequence.
        let message = doc.connect(&task, &order).unwrap();
        assert!(message.as_str().starts_with("MessageFlow"));
        assert_eq!(doc.message_flows().count(), 1);
        // Pool-to-pool works too; pool-to-own-content does not.
        assert!(doc.connect(&pool_b, &task).is_some());
        assert!(doc.connect(&pool_a, &task).is_none(), "same pool");

        // Moving pool B carries its content and the message flow endpoint.
        let moved: HashSet<BpmnId> = [pool_b.clone()].into_iter().collect();
        let before = doc.node_outline(&order).unwrap().1;
        doc.move_shapes(&moved, 50.0, 100.0);
        let after = doc.node_outline(&order).unwrap().1;
        assert_eq!((after.x - before.x, after.y - before.y), (50.0, 100.0));

        // Deleting pool B removes its process, content, and message flows.
        doc.delete_elements(&moved);
        assert!(doc.participant(&pool_b).is_none());
        assert_eq!(doc.message_flows().count(), 0);
        assert_eq!(doc.processes().count(), 1);

        // Deleting the last pool takes its contents, dissolves the
        // collaboration, and leaves a fresh empty process on the plane.
        let doomed: HashSet<BpmnId> = [pool_a].into_iter().collect();
        doc.delete_elements(&doomed);
        assert!(doc.collaboration().is_none());
        assert_eq!(doc.flow_elements().count(), 0);
        let process = doc.processes().next().unwrap();
        let plane_ref = doc.diagram().unwrap().plane.as_ref().unwrap();
        assert_eq!(plane_ref.bpmn_element, Some(process.base.id.clone()));
    }

    #[test]
    fn lanes_grow_the_pool_and_track_membership() {
        let mut doc = Document::new();
        let pool = doc.create_pool(None, Point::new(0.0, 0.0)).unwrap();
        let (_, before) = doc.node_outline(&pool).unwrap();

        let top = doc.add_lane(&pool, Some(String::from("Sales"))).unwrap();
        let bottom = doc.add_lane(&pool, Some(String::from("Support"))).unwrap();
        let (_, after) = doc.node_outline(&pool).unwrap();
        assert!(after.height > before.height, "second lane grew the pool");

        // A node placed in the top lane is referenced by it.
        let task = doc
            .create_node(NodeKind::Task(TaskType::User), Point::new(0.0, 0.0))
            .unwrap();
        assert_eq!(doc.lane(&top).unwrap().flow_node_refs, vec![task.clone()]);
        assert_eq!(doc.lane(&bottom).unwrap().flow_node_refs, Vec::new());

        // Moving it into the bottom lane moves the reference. (Lanes have no
        // outline — they are not connectable — so read their DI directly.)
        let shape_bounds = |doc: &Document, id: &BpmnId| {
            doc.diagram()
                .unwrap()
                .plane
                .as_ref()
                .unwrap()
                .elements
                .iter()
                .find_map(|e| match e {
                    DiElement::Shape(shape) if shape.bpmn_element == *id => Some(shape.bounds),
                    _ => None,
                })
                .unwrap()
        };
        let moved: HashSet<BpmnId> = [task.clone()].into_iter().collect();
        let bottom_bounds = shape_bounds(&doc, &bottom);
        let (_, task_bounds) = doc.node_outline(&task).unwrap();
        let dy = bottom_bounds.center().y - task_bounds.center().y;
        doc.move_shapes(&moved, 0.0, dy);
        assert_eq!(doc.lane(&top).unwrap().flow_node_refs, Vec::new());
        assert_eq!(doc.lane(&bottom).unwrap().flow_node_refs, vec![task]);
    }

    #[test]
    fn data_and_annotations_connect_with_associations() {
        let mut doc = Document::new();
        let task = doc
            .create_node(NodeKind::Task(TaskType::User), Point::new(0.0, 0.0))
            .unwrap();
        let data = doc
            .create_node(NodeKind::DataObject, Point::new(0.0, -200.0))
            .unwrap();
        let note = doc
            .create_node(NodeKind::TextAnnotation, Point::new(200.0, -200.0))
            .unwrap();

        // The reference got a backing dataObject declaration.
        assert_eq!(data.as_str(), "DataObjectReference_1");
        let index = doc.flow_element_index();
        assert!(matches!(
            index[&BpmnId::from("DataObject_1")],
            FlowElement::Data(DataElement {
                kind: DataKind::Object,
                ..
            })
        ));
        drop(index);

        // Connecting to data or annotations yields associations, not flows.
        let a1 = doc.connect(&task, &data).unwrap();
        let a2 = doc.connect(&note, &task).unwrap();
        assert!(a1.as_str().starts_with("Association"));
        assert!(a2.as_str().starts_with("Association"));

        // Annotation text edits through set_name.
        assert!(doc.set_name(&note, Some(String::from("check this"))));
        assert_eq!(doc.flow_element_index()[&note].name(), Some("check this"));

        // Deleting the task takes its associations along; the data and the
        // annotation stay.
        let doomed: HashSet<BpmnId> = [task].into_iter().collect();
        doc.delete_elements(&doomed);
        let index = doc.flow_element_index();
        assert!(!index.contains_key(&a1));
        assert!(!index.contains_key(&a2));
        assert!(index.contains_key(&data));
        assert!(index.contains_key(&note));
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
