//! The structured JSON shapes the read tools return.
//!
//! bpmn-core stays serde-free; these DTOs are the wire format, built by hand
//! from a document snapshot. Kinds travel as kebab-case strings so the tool
//! schemas stay flat.

use bpmn_core::model::{
    ActivityKind, ArtifactKind, Condition, DataKind, EventDefinitionKind, EventPosition,
    FlowElement, GatewayKind, LoopKind, SubProcessKind, TaskType,
};
use bpmn_core::{BpmnId, Document};
use schemars::JsonSchema;
use serde::Serialize;
use std::collections::BTreeMap;

/// The kebab-case kind of a flow element (also the `kind` accepted by the
/// create tools).
pub fn kind_label(element: &FlowElement) -> &'static str {
    match element {
        FlowElement::Event(event) => match event.position {
            EventPosition::Start { .. } => "start-event",
            EventPosition::End => "end-event",
            EventPosition::IntermediateCatch => "intermediate-catch-event",
            EventPosition::IntermediateThrow => "intermediate-throw-event",
            EventPosition::Boundary { .. } => "boundary-event",
        },
        FlowElement::Activity(activity) => match &activity.kind {
            ActivityKind::Task(task_type) => task_type_label(*task_type),
            ActivityKind::SubProcess(sub) => match sub.kind {
                SubProcessKind::Embedded => "subprocess",
                SubProcessKind::Event => "event-subprocess",
                SubProcessKind::Transaction => "transaction",
            },
            ActivityKind::CallActivity(_) => "call-activity",
        },
        FlowElement::Gateway(gateway) => match gateway.kind {
            GatewayKind::Exclusive => "exclusive-gateway",
            GatewayKind::Parallel => "parallel-gateway",
            GatewayKind::Inclusive => "inclusive-gateway",
            GatewayKind::EventBased => "event-based-gateway",
            GatewayKind::Complex => "complex-gateway",
        },
        FlowElement::SequenceFlow(_) => "sequence-flow",
        FlowElement::Data(data) => match data.kind {
            DataKind::Object => "data-object-declaration",
            DataKind::ObjectReference { .. } => "data-object",
            DataKind::StoreReference { .. } => "data-store",
        },
        FlowElement::Artifact(artifact) => match artifact.kind {
            ArtifactKind::TextAnnotation { .. } => "text-annotation",
            ArtifactKind::Association { .. } => "association",
            ArtifactKind::Group { .. } => "group",
        },
        FlowElement::Unknown(_) => "unknown",
    }
}

/// The kebab-case label of a task type.
pub fn task_type_label(task_type: TaskType) -> &'static str {
    match task_type {
        TaskType::Generic => "task",
        TaskType::User => "user-task",
        TaskType::Service => "service-task",
        TaskType::Script => "script-task",
        TaskType::Manual => "manual-task",
        TaskType::Send => "send-task",
        TaskType::Receive => "receive-task",
        TaskType::BusinessRule => "business-rule-task",
    }
}

/// The kebab-case label of an event trigger.
pub fn trigger_label(kind: EventDefinitionKind) -> &'static str {
    match kind {
        EventDefinitionKind::Message => "message",
        EventDefinitionKind::Timer => "timer",
        EventDefinitionKind::Signal => "signal",
        EventDefinitionKind::Error => "error",
        EventDefinitionKind::Escalation => "escalation",
        EventDefinitionKind::Conditional => "conditional",
        EventDefinitionKind::Compensate => "compensate",
        EventDefinitionKind::Link => "link",
        EventDefinitionKind::Cancel => "cancel",
        EventDefinitionKind::Terminate => "terminate",
        EventDefinitionKind::Other => "other",
    }
}

/// The kebab-case label of a loop marker.
pub fn loop_label(kind: LoopKind) -> &'static str {
    match kind {
        LoopKind::Standard => "standard",
        LoopKind::MultiInstanceParallel => "multi-instance-parallel",
        LoopKind::MultiInstanceSequential => "multi-instance-sequential",
    }
}

fn condition_text(condition: Option<&Condition>) -> Option<String> {
    match condition? {
        Condition::Expression { text, .. } => Some(text.clone()),
        Condition::Raw(_) => None,
    }
}

/// One process of the document.
#[derive(Serialize, JsonSchema)]
pub struct ProcessSummary {
    /// The process id.
    pub id: String,
    /// The process name, when set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// One lane of a pool.
#[derive(Serialize, JsonSchema)]
pub struct LaneSummary {
    /// The lane id.
    pub id: String,
    /// The lane name, when set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// One pool (participant) of the collaboration.
#[derive(Serialize, JsonSchema)]
pub struct PoolSummary {
    /// The participant id.
    pub id: String,
    /// The pool name, when set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The process backing the pool, when it is not a black box.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub process: Option<String>,
    /// The pool's lanes, top to bottom.
    pub lanes: Vec<LaneSummary>,
}

/// Validation issue counts by severity.
#[derive(Serialize, JsonSchema, Default)]
pub struct IssueCounts {
    /// Blocking problems.
    pub errors: usize,
    /// Suspicious but legal constructs.
    pub warnings: usize,
    /// Informational notes.
    pub infos: usize,
}

/// A birds-eye view of the document.
#[derive(Serialize, JsonSchema)]
pub struct Overview {
    /// Every process.
    pub processes: Vec<ProcessSummary>,
    /// Every pool, when the document is a collaboration.
    pub pools: Vec<PoolSummary>,
    /// How many elements of each kind exist.
    pub element_counts: BTreeMap<&'static str, usize>,
    /// Validation issue counts.
    pub issues: IssueCounts,
}

/// One node of the diagram graph.
#[derive(Serialize, JsonSchema)]
pub struct GraphNode {
    /// The element id.
    pub id: String,
    /// The element kind (kebab-case).
    pub kind: &'static str,
    /// The element name, when set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The process or subprocess directly containing the element.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub container: Option<String>,
    /// Center x in diagram units.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub x: Option<f64>,
    /// Center y in diagram units.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub y: Option<f64>,
    /// Shape width.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<f64>,
    /// Shape height.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<f64>,
    /// An event's trigger, when it has one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_trigger: Option<&'static str>,
    /// The activity a boundary event is attached to.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attached_to: Option<String>,
    /// Whether the boundary event interrupts its activity.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cancel_activity: Option<bool>,
    /// A gateway's default flow.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gateway_default: Option<String>,
    /// Whether a subprocess is drawn expanded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expanded: Option<bool>,
    /// A call activity's target.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub called_element: Option<String>,
    /// An activity's loop marker.
    #[serde(rename = "loop", skip_serializing_if = "Option::is_none")]
    pub loop_marker: Option<&'static str>,
    /// A text annotation's text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

/// One edge of the diagram graph.
#[derive(Serialize, JsonSchema)]
pub struct GraphEdge {
    /// The flow id.
    pub id: String,
    /// `sequence`, `message`, or `association`.
    pub kind: &'static str,
    /// The source element.
    pub source: String,
    /// The target element.
    pub target: String,
    /// The flow label, when set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// A sequence flow's condition expression.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub condition: Option<String>,
}

/// One lane and its members.
#[derive(Serialize, JsonSchema)]
pub struct GraphLane {
    /// The lane id.
    pub id: String,
    /// The lane name, when set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The pool the lane belongs to.
    pub pool: String,
    /// The node ids inside the lane.
    pub members: Vec<String>,
}

/// The document (or one container of it) as nodes and the links between them.
#[derive(Serialize, JsonSchema)]
pub struct Graph {
    /// The container the graph was filtered to, when one was given.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub container: Option<String>,
    /// The nodes.
    pub nodes: Vec<GraphNode>,
    /// The links: sequence flows, message flows, and associations.
    pub edges: Vec<GraphEdge>,
    /// Lanes and their members.
    pub lanes: Vec<GraphLane>,
}

/// A flow touching the described element.
#[derive(Serialize, JsonSchema)]
pub struct FlowRef {
    /// The flow id.
    pub id: String,
    /// `sequence`, `message`, or `association`.
    pub kind: &'static str,
    /// The element on the other end.
    pub other: String,
    /// The flow label, when set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// A sequence flow's condition expression.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub condition: Option<String>,
}

/// Everything known about one element.
#[derive(Serialize, JsonSchema)]
pub struct ElementDetail {
    /// The node, with its condensed properties.
    #[serde(flatten)]
    pub node: GraphNode,
    /// Containers from the process down to the direct parent.
    pub container_chain: Vec<String>,
    /// The lane holding the element, when pools have lanes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lane: Option<String>,
    /// Flows arriving at the element.
    pub incoming: Vec<FlowRef>,
    /// Flows leaving the element.
    pub outgoing: Vec<FlowRef>,
    /// Boundary events attached to the element.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub boundary_events: Vec<String>,
    /// The element's documentation entries.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub documentation: Vec<String>,
}

/// One validation issue.
#[derive(Serialize, JsonSchema)]
pub struct IssueDto {
    /// `error`, `warning`, or `info`.
    pub severity: &'static str,
    /// The rule that fired.
    pub rule: String,
    /// What is wrong.
    pub message: String,
    /// The offending element, when the rule points at one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub element: Option<String>,
}

/// The validation result.
#[derive(Serialize, JsonSchema)]
pub struct ValidationReport {
    /// Every issue found.
    pub issues: Vec<IssueDto>,
    /// Counts by severity.
    pub counts: IssueCounts,
}

/// The result of creating one element.
#[derive(Serialize, JsonSchema)]
pub struct CreateResult {
    /// The new element's id.
    pub id: String,
}

/// The result of `create_node`.
#[derive(Serialize, JsonSchema)]
pub struct CreateNodeResult {
    /// The new node's id.
    pub id: String,
    /// The connecting flow, when `after` was given.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub flow: Option<String>,
}

/// The result of `create_sequence`.
#[derive(Serialize, JsonSchema)]
pub struct CreateSequenceResult {
    /// The created nodes, in chain order.
    pub nodes: Vec<String>,
    /// The created flows, in chain order.
    pub flows: Vec<String>,
}

/// One refused field of an `update_element` call.
#[derive(Serialize, JsonSchema)]
pub struct FieldFailure {
    /// The field name.
    pub field: &'static str,
    /// Why it did not apply.
    pub reason: String,
}

/// The result of `update_element`.
#[derive(Serialize, JsonSchema)]
pub struct UpdateResult {
    /// The fields that changed.
    pub applied: Vec<&'static str>,
    /// The fields that did not, with reasons.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub failed: Vec<FieldFailure>,
}

/// The result of a bulk or whole-document change.
#[derive(Serialize, JsonSchema)]
pub struct ChangeResult {
    /// Whether the document changed.
    pub changed: bool,
    /// The ids acted on, for bulk actions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub affected: Option<Vec<String>>,
    /// The ids the document does not know.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unknown: Option<Vec<String>>,
}

fn ids(list: &[BpmnId]) -> Vec<String> {
    list.iter().map(|id| id.as_str().to_string()).collect()
}

/// Builds a [`GraphNode`] for one flow element.
fn graph_node(document: &Document, element: &FlowElement, id: &BpmnId) -> GraphNode {
    let bounds = document.node_outline(id).map(|(_, b)| b);
    let mut node = GraphNode {
        id: id.as_str().to_string(),
        kind: kind_label(element),
        name: element.name().map(str::to_string),
        container: document.container_of(id).map(|c| c.as_str().to_string()),
        x: bounds.map(|b| b.center().x),
        y: bounds.map(|b| b.center().y),
        width: bounds.map(|b| b.width),
        height: bounds.map(|b| b.height),
        event_trigger: None,
        attached_to: None,
        cancel_activity: None,
        gateway_default: None,
        expanded: None,
        called_element: None,
        loop_marker: None,
        text: None,
    };
    match element {
        FlowElement::Event(event) => {
            node.event_trigger = event.definitions.first().map(|d| trigger_label(d.kind));
            if let EventPosition::Boundary {
                attached_to,
                cancel_activity,
            } = &event.position
            {
                node.attached_to = Some(attached_to.as_str().to_string());
                node.cancel_activity = *cancel_activity;
            }
        }
        FlowElement::Activity(activity) => {
            node.loop_marker = activity.loop_marker.as_ref().map(|m| loop_label(m.kind));
            match &activity.kind {
                ActivityKind::SubProcess(_) => {
                    node.expanded = Some(subprocess_is_expanded(document, id));
                }
                ActivityKind::CallActivity(call) => {
                    node.called_element.clone_from(&call.called_element);
                }
                ActivityKind::Task(_) => {}
            }
        }
        FlowElement::Gateway(gateway) => {
            node.gateway_default = gateway
                .default_flow
                .as_ref()
                .map(|f| f.as_str().to_string());
        }
        FlowElement::Artifact(artifact) => {
            if let ArtifactKind::TextAnnotation { text } = &artifact.kind {
                node.text.clone_from(text);
            }
        }
        _ => {}
    }
    node
}

/// Whether the subprocess's shape carries `isExpanded="true"`.
fn subprocess_is_expanded(document: &Document, id: &BpmnId) -> bool {
    use bpmn_core::di::DiElement;
    document.definitions.diagrams.iter().any(|diagram| {
        diagram.plane.as_ref().is_some_and(|plane| {
            plane.elements.iter().any(|element| {
                matches!(element, DiElement::Shape(shape)
                    if shape.bpmn_element == *id && shape.is_expanded == Some(true))
            })
        })
    })
}

/// Builds the overview.
pub fn overview(document: &Document, issues: &[bpmn_validate::Issue]) -> Overview {
    let mut element_counts: BTreeMap<&'static str, usize> = BTreeMap::new();
    for element in document.flow_elements() {
        *element_counts.entry(kind_label(element)).or_default() += 1;
    }
    let pools = document
        .collaboration()
        .map(|collaboration| {
            collaboration
                .participants
                .iter()
                .map(|participant| PoolSummary {
                    id: participant.base.id.as_str().to_string(),
                    name: participant.name.clone(),
                    process: participant
                        .process_ref
                        .as_ref()
                        .map(|p| p.as_str().to_string()),
                    lanes: lanes_of(document, participant)
                        .map(|lane| LaneSummary {
                            id: lane.base.id.as_str().to_string(),
                            name: lane.name.clone(),
                        })
                        .collect(),
                })
                .collect()
        })
        .unwrap_or_default();
    Overview {
        processes: document
            .processes()
            .map(|process| ProcessSummary {
                id: process.base.id.as_str().to_string(),
                name: process.name.clone(),
            })
            .collect(),
        pools,
        element_counts,
        issues: issue_counts(issues),
    }
}

fn lanes_of<'a>(
    document: &'a Document,
    participant: &bpmn_core::model::Participant,
) -> impl Iterator<Item = &'a bpmn_core::model::Lane> {
    document
        .processes()
        .filter(move |process| Some(&process.base.id) == participant.process_ref.as_ref())
        .flat_map(|process| process.lane_sets.iter())
        .flat_map(|set| set.lanes.iter())
}

fn issue_counts(issues: &[bpmn_validate::Issue]) -> IssueCounts {
    let mut counts = IssueCounts::default();
    for issue in issues {
        match issue.severity {
            bpmn_validate::Severity::Error => counts.errors += 1,
            bpmn_validate::Severity::Warning => counts.warnings += 1,
            bpmn_validate::Severity::Info => counts.infos += 1,
        }
    }
    counts
}

/// Builds the graph, optionally filtered to one container's direct contents.
pub fn graph(document: &Document, container: Option<&BpmnId>) -> Graph {
    let keep = |id: &BpmnId| -> bool {
        match container {
            None => true,
            Some(container) => document.container_of(id).as_ref() == Some(container),
        }
    };

    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    let mut in_set: std::collections::HashSet<&BpmnId> = std::collections::HashSet::new();
    for element in document.flow_elements() {
        let Some(id) = element.id() else { continue };
        if !keep(id) {
            continue;
        }
        match element {
            FlowElement::SequenceFlow(flow) => edges.push(GraphEdge {
                id: id.as_str().to_string(),
                kind: "sequence",
                source: flow.source.as_str().to_string(),
                target: flow.target.as_str().to_string(),
                name: flow.name.clone(),
                condition: condition_text(flow.condition.as_ref()),
            }),
            FlowElement::Artifact(bpmn_core::model::Artifact {
                kind: ArtifactKind::Association { source, target, .. },
                ..
            }) => edges.push(GraphEdge {
                id: id.as_str().to_string(),
                kind: "association",
                source: source.as_str().to_string(),
                target: target.as_str().to_string(),
                name: None,
                condition: None,
            }),
            _ => {
                in_set.insert(id);
                nodes.push(graph_node(document, element, id));
            }
        }
    }
    if container.is_none() {
        for flow in document.message_flows() {
            edges.push(GraphEdge {
                id: flow.base.id.as_str().to_string(),
                kind: "message",
                source: flow.source.as_str().to_string(),
                target: flow.target.as_str().to_string(),
                name: flow.name.clone(),
                condition: None,
            });
        }
    }

    let mut lanes = Vec::new();
    if container.is_none()
        && let Some(collaboration) = document.collaboration()
    {
        for participant in &collaboration.participants {
            for lane in lanes_of(document, participant) {
                lanes.push(GraphLane {
                    id: lane.base.id.as_str().to_string(),
                    name: lane.name.clone(),
                    pool: participant.base.id.as_str().to_string(),
                    members: ids(&lane.flow_node_refs),
                });
            }
        }
    }

    Graph {
        container: container.map(|c| c.as_str().to_string()),
        nodes,
        edges,
        lanes,
    }
}

/// Builds the full detail of one element.
pub fn describe(document: &Document, id: &BpmnId) -> Option<ElementDetail> {
    let index = document.flow_element_index();
    let element = index.get(id).copied()?;
    let node = graph_node(document, element, id);

    let mut container_chain = Vec::new();
    let mut cursor = id.clone();
    while let Some(container) = document.container_of(&cursor) {
        container_chain.push(container.as_str().to_string());
        cursor = container;
    }
    container_chain.reverse();

    let flow_ref = |flow_id: &BpmnId, other: &BpmnId, kind: &'static str| FlowRef {
        id: flow_id.as_str().to_string(),
        kind,
        other: other.as_str().to_string(),
        name: index
            .get(flow_id)
            .and_then(|e| e.name())
            .map(str::to_string),
        condition: match index.get(flow_id) {
            Some(FlowElement::SequenceFlow(flow)) => condition_text(flow.condition.as_ref()),
            _ => None,
        },
    };

    let mut incoming = Vec::new();
    let mut outgoing = Vec::new();
    for element in document.flow_elements() {
        match element {
            FlowElement::SequenceFlow(flow) => {
                if flow.target == *id {
                    incoming.push(flow_ref(&flow.base.id, &flow.source, "sequence"));
                }
                if flow.source == *id {
                    outgoing.push(flow_ref(&flow.base.id, &flow.target, "sequence"));
                }
            }
            FlowElement::Artifact(bpmn_core::model::Artifact {
                base,
                kind: ArtifactKind::Association { source, target, .. },
            }) => {
                if *target == *id {
                    incoming.push(flow_ref(&base.id, source, "association"));
                }
                if *source == *id {
                    outgoing.push(flow_ref(&base.id, target, "association"));
                }
            }
            _ => {}
        }
    }
    for flow in document.message_flows() {
        if flow.target == *id {
            incoming.push(FlowRef {
                id: flow.base.id.as_str().to_string(),
                kind: "message",
                other: flow.source.as_str().to_string(),
                name: flow.name.clone(),
                condition: None,
            });
        }
        if flow.source == *id {
            outgoing.push(FlowRef {
                id: flow.base.id.as_str().to_string(),
                kind: "message",
                other: flow.target.as_str().to_string(),
                name: flow.name.clone(),
                condition: None,
            });
        }
    }

    let boundary_events = document
        .flow_elements()
        .filter_map(|element| match element {
            FlowElement::Event(event) => match &event.position {
                EventPosition::Boundary { attached_to, .. } if attached_to == id => {
                    Some(event.node.base.id.as_str().to_string())
                }
                _ => None,
            },
            _ => None,
        })
        .collect();

    let lane = document
        .processes()
        .flat_map(|process| process.lane_sets.iter())
        .flat_map(|set| set.lanes.iter())
        .find(|lane| lane.flow_node_refs.contains(id))
        .map(|lane| lane.base.id.as_str().to_string());

    let documentation = documentation_of(element);

    Some(ElementDetail {
        node,
        container_chain,
        lane,
        incoming,
        outgoing,
        boundary_events,
        documentation,
    })
}

fn documentation_of(element: &FlowElement) -> Vec<String> {
    let base = match element {
        FlowElement::Event(event) => &event.node.base,
        FlowElement::Activity(activity) => &activity.node.base,
        FlowElement::Gateway(gateway) => &gateway.node.base,
        FlowElement::SequenceFlow(flow) => &flow.base,
        FlowElement::Data(data) => &data.base,
        FlowElement::Artifact(artifact) => &artifact.base,
        FlowElement::Unknown(_) => return Vec::new(),
    };
    base.documentation.clone()
}

/// Builds the validation report.
pub fn validation(issues: Vec<bpmn_validate::Issue>) -> ValidationReport {
    let counts = issue_counts(&issues);
    ValidationReport {
        issues: issues
            .into_iter()
            .map(|issue| IssueDto {
                severity: match issue.severity {
                    bpmn_validate::Severity::Error => "error",
                    bpmn_validate::Severity::Warning => "warning",
                    bpmn_validate::Severity::Info => "info",
                },
                rule: issue.rule.to_string(),
                message: issue.message,
                element: issue.element.map(|id| id.as_str().to_string()),
            })
            .collect(),
        counts,
    }
}

/// The ids of an outcome's affected/unknown lists, as a [`ChangeResult`].
pub fn change_result(outcome: &crate::bridge::ApplyOutcome) -> ChangeResult {
    match outcome {
        crate::bridge::ApplyOutcome::Affected { affected, unknown } => ChangeResult {
            changed: !affected.is_empty(),
            affected: Some(ids(affected)),
            unknown: Some(ids(unknown)),
        },
        other => ChangeResult {
            changed: other.changed_document(),
            affected: None,
            unknown: None,
        },
    }
}
