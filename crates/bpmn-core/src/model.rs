//! The BPMN 2.0 semantic model (M1 subset).
//!
//! Shape of the model: nested structs for data shared along the BPMN class
//! hierarchy (`BaseElement` → `FlowNode`) plus enums for variation
//! (`FlowElement`, `EventPosition`, `TaskType`, ...). Enums give exhaustive
//! `match`es — the compiler points at every renderer/validator/serializer that
//! must learn about a new element kind — and cheap `Clone` for snapshot undo.
//!
//! Elements outside the modeled subset survive as [`RawXml`] in `Unknown`
//! variants, so every file round-trips even before full coverage lands.

use crate::ids::BpmnId;
use crate::raw::RawXml;

/// Data every BPMN element carries (`BaseElement` in the spec).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BaseElement {
    /// The XML `id` attribute.
    pub id: BpmnId,
    /// `documentation` child elements (text content).
    pub documentation: Vec<String>,
    /// The whole `extensionElements` element, preserved verbatim.
    pub extension_elements: Option<RawXml>,
    /// Attributes from foreign namespaces (`camunda:*`, ...), as written.
    pub unknown_attrs: Vec<(String, String)>,
    /// Child elements the model does not understand, preserved verbatim.
    pub unknown_children: Vec<RawXml>,
}

impl BaseElement {
    /// Creates a base element with just an id.
    pub fn new(id: BpmnId) -> Self {
        Self {
            id,
            ..Self::default()
        }
    }
}

/// Data shared by all flow nodes (events, activities, gateways).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FlowNode {
    /// Common element data.
    pub base: BaseElement,
    /// The `name` attribute (the label on the diagram).
    pub name: Option<String>,
    /// `incoming` child elements (sequence flow ids), as written.
    pub incoming: Vec<BpmnId>,
    /// `outgoing` child elements (sequence flow ids), as written.
    pub outgoing: Vec<BpmnId>,
}

/// One element inside a process's flow-element collection.
#[derive(Debug, Clone, PartialEq)]
pub enum FlowElement {
    /// An event (start, end, intermediate, boundary).
    Event(Event),
    /// An activity (a task, subprocess, or call activity).
    Activity(Activity),
    /// A gateway.
    Gateway(Gateway),
    /// A sequence flow connecting two flow nodes.
    SequenceFlow(SequenceFlow),
    /// Anything not modeled yet (subprocesses, data objects, lanes, ...).
    Unknown(RawXml),
}

impl FlowElement {
    /// The element's id, when it is a modeled element.
    pub fn id(&self) -> Option<&BpmnId> {
        match self {
            FlowElement::Event(e) => Some(&e.node.base.id),
            FlowElement::Activity(a) => Some(&a.node.base.id),
            FlowElement::Gateway(g) => Some(&g.node.base.id),
            FlowElement::SequenceFlow(f) => Some(&f.base.id),
            FlowElement::Unknown(_) => None,
        }
    }

    /// The element's display name, when it has one.
    pub fn name(&self) -> Option<&str> {
        match self {
            FlowElement::Event(e) => e.node.name.as_deref(),
            FlowElement::Activity(a) => a.node.name.as_deref(),
            FlowElement::Gateway(g) => g.node.name.as_deref(),
            FlowElement::SequenceFlow(f) => f.name.as_deref(),
            FlowElement::Unknown(_) => None,
        }
    }
}

/// An event. Two orthogonal axes: where it sits ([`EventPosition`]) and what
/// triggers/results it carries ([`EventDefinition`]).
#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    /// Flow node data.
    pub node: FlowNode,
    /// Start / end / intermediate / boundary.
    pub position: EventPosition,
    /// Zero (plain), one (typed), or several (multiple) definitions.
    pub definitions: Vec<EventDefinition>,
}

/// Where an event sits in the process.
#[derive(Debug, Clone, PartialEq)]
pub enum EventPosition {
    /// `startEvent`.
    Start {
        /// The `isInterrupting` attribute (absent means `true`).
        is_interrupting: Option<bool>,
    },
    /// `endEvent`.
    End,
    /// `intermediateCatchEvent`.
    IntermediateCatch,
    /// `intermediateThrowEvent`.
    IntermediateThrow,
    /// `boundaryEvent`, attached to an activity.
    Boundary {
        /// The `attachedToRef` attribute.
        attached_to: BpmnId,
        /// The `cancelActivity` attribute (absent means `true`).
        cancel_activity: Option<bool>,
    },
}

/// What an event reacts to or produces: the trigger kind (always known, it
/// drives the icon) plus the definition body (structured when simple,
/// verbatim otherwise).
#[derive(Debug, Clone, PartialEq)]
pub struct EventDefinition {
    /// Which trigger this is, from the XML tag name.
    pub kind: EventDefinitionKind,
    /// The definition's content.
    pub body: EventDefinitionBody,
}

/// The trigger kinds of BPMN 2.0, by XML tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventDefinitionKind {
    /// `messageEventDefinition`.
    Message,
    /// `timerEventDefinition`.
    Timer,
    /// `signalEventDefinition`.
    Signal,
    /// `errorEventDefinition`.
    Error,
    /// `escalationEventDefinition`.
    Escalation,
    /// `conditionalEventDefinition`.
    Conditional,
    /// `compensateEventDefinition`.
    Compensate,
    /// `linkEventDefinition`.
    Link,
    /// `cancelEventDefinition`.
    Cancel,
    /// `terminateEventDefinition`.
    Terminate,
    /// A definition tag we do not recognize.
    Other,
}

impl EventDefinitionKind {
    /// The XML local tag name, when the kind maps to one.
    pub fn tag(self) -> Option<&'static str> {
        Some(match self {
            Self::Message => "messageEventDefinition",
            Self::Timer => "timerEventDefinition",
            Self::Signal => "signalEventDefinition",
            Self::Error => "errorEventDefinition",
            Self::Escalation => "escalationEventDefinition",
            Self::Conditional => "conditionalEventDefinition",
            Self::Compensate => "compensateEventDefinition",
            Self::Link => "linkEventDefinition",
            Self::Cancel => "cancelEventDefinition",
            Self::Terminate => "terminateEventDefinition",
            Self::Other => return None,
        })
    }
}

/// An event definition's content.
#[derive(Debug, Clone, PartialEq)]
pub enum EventDefinitionBody {
    /// A message definition the editor understands.
    Message {
        /// Id of the element itself, if present.
        id: Option<BpmnId>,
        /// The `messageRef` attribute.
        message_ref: Option<String>,
    },
    /// A bare definition element with no content beyond an optional id
    /// (what the editor creates for timers, signals, terminate, ...).
    Empty {
        /// Id of the element itself, if present.
        id: Option<BpmnId>,
    },
    /// Anything richer (timer durations, error refs, vendor content),
    /// preserved verbatim.
    Raw(RawXml),
}

/// An activity.
#[derive(Debug, Clone, PartialEq)]
pub struct Activity {
    /// Flow node data.
    pub node: FlowNode,
    /// What kind of activity this is.
    pub kind: ActivityKind,
    /// Loop / multi-instance marker, when present.
    pub loop_marker: Option<LoopMarker>,
}

/// A loop or multi-instance characteristic on an activity.
#[derive(Debug, Clone, PartialEq)]
pub struct LoopMarker {
    /// Which marker is drawn.
    pub kind: LoopKind,
    /// The original element, verbatim, when it carried more than the editor
    /// understands (cardinalities, completion conditions, vendor content).
    pub raw: Option<RawXml>,
}

/// The loop marker kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopKind {
    /// `standardLoopCharacteristics`.
    Standard,
    /// `multiInstanceLoopCharacteristics` (parallel, the default).
    MultiInstanceParallel,
    /// `multiInstanceLoopCharacteristics isSequential="true"`.
    MultiInstanceSequential,
}

/// The kinds of activity.
#[derive(Debug, Clone, PartialEq)]
pub enum ActivityKind {
    /// One of the eight task types.
    Task(TaskType),
    /// An embedded subprocess, event subprocess, or transaction.
    SubProcess(SubProcess),
    /// A `callActivity` invoking another process.
    CallActivity(CallActivity),
}

/// A subprocess: an activity that owns its own flow elements.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SubProcess {
    /// Which container flavor this is (it decides the XML tag and border).
    pub kind: SubProcessKind,
    /// Nested flow elements in document order, like a process body.
    pub flow_elements: Vec<FlowElement>,
}

/// The subprocess flavors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SubProcessKind {
    /// `subProcess`.
    #[default]
    Embedded,
    /// `subProcess triggeredByEvent="true"` — started by its own events,
    /// never by sequence flow.
    Event,
    /// `transaction`.
    Transaction,
}

/// A `callActivity`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CallActivity {
    /// The `calledElement` attribute (the id of the invoked process).
    pub called_element: Option<String>,
}

/// BPMN task types, by XML tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskType {
    /// `task` — the generic, untyped task.
    Generic,
    /// `userTask`.
    User,
    /// `serviceTask`.
    Service,
    /// `scriptTask`.
    Script,
    /// `manualTask`.
    Manual,
    /// `sendTask`.
    Send,
    /// `receiveTask`.
    Receive,
    /// `businessRuleTask`.
    BusinessRule,
}

/// A gateway.
#[derive(Debug, Clone, PartialEq)]
pub struct Gateway {
    /// Flow node data.
    pub node: FlowNode,
    /// What kind of gateway this is.
    pub kind: GatewayKind,
    /// The `default` attribute (default sequence flow), on exclusive,
    /// inclusive, and complex gateways.
    pub default_flow: Option<BpmnId>,
    /// The `isMarkerVisible` attribute of the DI shape lives in DI; this is
    /// the semantic `instantiate` attribute of event-based gateways.
    pub instantiate: Option<bool>,
}

/// The kinds of gateway, by XML tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GatewayKind {
    /// `exclusiveGateway`.
    Exclusive,
    /// `parallelGateway`.
    Parallel,
    /// `inclusiveGateway`.
    Inclusive,
    /// `eventBasedGateway`.
    EventBased,
    /// `complexGateway`.
    Complex,
}

/// A sequence flow connecting two flow nodes.
#[derive(Debug, Clone, PartialEq)]
pub struct SequenceFlow {
    /// Common element data.
    pub base: BaseElement,
    /// The `name` attribute (the label on the diagram).
    pub name: Option<String>,
    /// The `sourceRef` attribute.
    pub source: BpmnId,
    /// The `targetRef` attribute.
    pub target: BpmnId,
    /// The `conditionExpression` child.
    pub condition: Option<Condition>,
}

/// A sequence flow condition.
#[derive(Debug, Clone, PartialEq)]
pub enum Condition {
    /// A text-only `conditionExpression`, editable in the properties panel.
    Expression {
        /// The `xsi:type` attribute exactly as written (usually
        /// `bpmn:tFormalExpression`), kept so files round-trip.
        xsi_type: Option<String>,
        /// The expression text.
        text: String,
    },
    /// A condition with structure we don't model, preserved verbatim.
    Raw(RawXml),
}

/// A `process` root element.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Process {
    /// Common element data.
    pub base: BaseElement,
    /// The `name` attribute.
    pub name: Option<String>,
    /// The `isExecutable` attribute, as written (absent stays absent).
    pub is_executable: Option<bool>,
    /// `laneSet` children (usually one), written before the flow elements.
    pub lane_sets: Vec<LaneSet>,
    /// Flow elements in document order. Unmodeled children (artifacts, data
    /// objects, ...) are interleaved as [`FlowElement::Unknown`] so order
    /// survives the round-trip.
    pub flow_elements: Vec<FlowElement>,
}

/// A `laneSet`: the lanes partitioning a process (or a lane, when nested).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LaneSet {
    /// The `id` attribute.
    pub id: Option<BpmnId>,
    /// The lanes, in document order.
    pub lanes: Vec<Lane>,
    /// Attributes the model does not understand, as written.
    pub unknown_attrs: Vec<(String, String)>,
}

/// A `lane`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Lane {
    /// Common element data.
    pub base: BaseElement,
    /// The `name` attribute (the lane's label).
    pub name: Option<String>,
    /// `flowNodeRef` children: the ids of the nodes living in this lane.
    pub flow_node_refs: Vec<BpmnId>,
    /// A `childLaneSet`, when the lane is subdivided further.
    pub child_lane_set: Option<LaneSet>,
}

/// A `collaboration` root element: pools and the message flows between them.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Collaboration {
    /// Common element data.
    pub base: BaseElement,
    /// The `name` attribute.
    pub name: Option<String>,
    /// The pools.
    pub participants: Vec<Participant>,
    /// The message flows between pools.
    pub message_flows: Vec<MessageFlow>,
}

/// A `participant`: one pool, usually backed by a process.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Participant {
    /// Common element data.
    pub base: BaseElement,
    /// The `name` attribute (the pool's label).
    pub name: Option<String>,
    /// The `processRef` attribute. A pool without one is a black box.
    pub process_ref: Option<BpmnId>,
}

/// A `messageFlow` connecting two pools (or flow nodes in different pools).
#[derive(Debug, Clone, PartialEq)]
pub struct MessageFlow {
    /// Common element data.
    pub base: BaseElement,
    /// The `name` attribute (the label on the diagram).
    pub name: Option<String>,
    /// The `sourceRef` attribute: a flow node or participant id.
    pub source: BpmnId,
    /// The `targetRef` attribute: a flow node or participant id.
    pub target: BpmnId,
}

/// A direct child of `definitions`.
#[derive(Debug, Clone, PartialEq)]
pub enum RootElement {
    /// A `process`.
    Process(Process),
    /// A `collaboration`.
    Collaboration(Collaboration),
    /// Anything else (messages, signals, error definitions, ...), verbatim.
    Unknown(RawXml),
}

/// The `targetNamespace` boss stamps on every document it saves. Only the
/// ISO/OMG namespaces carry meaning for boss; whatever `targetNamespace`
/// another tool wrote is replaced by this one.
pub const BOSS_TARGET_NAMESPACE: &str = "https://github.com/csaldiasdev/boss/schema/bpmn";

/// The `definitions` document root.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Definitions {
    /// The `id` attribute.
    pub id: Option<BpmnId>,
    /// The `targetNamespace` attribute.
    pub target_namespace: String,
    /// The `exporter` attribute, preserved from the source file.
    pub exporter: Option<String>,
    /// The `exporterVersion` attribute, preserved from the source file.
    pub exporter_version: Option<String>,
    /// Every namespace declaration on the root, in source order
    /// (`None` prefix = default `xmlns`). Preserved so raw fragments keep
    /// their prefixes working; the writer only appends, never removes.
    pub ns_decls: Vec<(Option<String>, String)>,
    /// Other root attributes (`xsi:schemaLocation`, vendor attributes, ...).
    pub unknown_attrs: Vec<(String, String)>,
    /// Semantic root elements in document order.
    pub root_elements: Vec<RootElement>,
    /// BPMN DI diagrams (siblings of the semantic model, never embedded).
    pub diagrams: Vec<crate::di::BpmnDiagram>,
}
