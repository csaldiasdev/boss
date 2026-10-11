//! Validation rule engine for BPMN 2.0 documents.
//!
//! Headless (no UI dependencies) so the editor, a CLI linter, and the MCP
//! server can all run the same rules. Rules are graph-local and documents are
//! small, so validation is a full pass over a prebuilt [`Graph`].

use bpmn_core::model::{
    ActivityKind, Condition, EventPosition, FlowElement, Gateway, GatewayKind, SequenceFlow,
    SubProcessKind, TaskType,
};
use bpmn_core::{BpmnId, Document};
use std::collections::{HashMap, HashSet};

/// How bad an issue is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// Advice.
    Info,
    /// Probably a mistake; the file still makes sense.
    Warning,
    /// The process is structurally broken.
    Error,
}

/// One finding.
#[derive(Debug, Clone, PartialEq)]
pub struct Issue {
    /// Stable rule identifier (`gateway-no-outgoing`, ...).
    pub rule: &'static str,
    /// How bad it is.
    pub severity: Severity,
    /// The element at fault, when the issue anchors to one.
    pub element: Option<BpmnId>,
    /// Human-readable explanation.
    pub message: String,
}

/// Connectivity derived from sequence flows (the source of truth — the
/// `incoming`/`outgoing` hints in the XML are not trusted).
pub struct Graph<'a> {
    document: &'a Document,
    index: HashMap<&'a BpmnId, &'a FlowElement>,
    incoming: HashMap<&'a BpmnId, Vec<&'a SequenceFlow>>,
    outgoing: HashMap<&'a BpmnId, Vec<&'a SequenceFlow>>,
}

impl<'a> Graph<'a> {
    fn new(document: &'a Document) -> Self {
        let index = document.flow_element_index();
        let mut incoming: HashMap<&BpmnId, Vec<&SequenceFlow>> = HashMap::new();
        let mut outgoing: HashMap<&BpmnId, Vec<&SequenceFlow>> = HashMap::new();

        for element in document.flow_elements() {
            if let FlowElement::SequenceFlow(flow) = element {
                if let Some((source, _)) = index.get_key_value(&flow.source) {
                    outgoing.entry(source).or_default().push(flow);
                }
                if let Some((target, _)) = index.get_key_value(&flow.target) {
                    incoming.entry(target).or_default().push(flow);
                }
            }
        }

        Self {
            document,
            index,
            incoming,
            outgoing,
        }
    }

    fn incoming(&self, id: &BpmnId) -> &[&'a SequenceFlow] {
        self.incoming.get(id).map_or(&[], Vec::as_slice)
    }

    fn outgoing(&self, id: &BpmnId) -> &[&'a SequenceFlow] {
        self.outgoing.get(id).map_or(&[], Vec::as_slice)
    }
}

/// A validation rule.
pub trait Rule: Send + Sync {
    /// Stable identifier, also shown in the issues panel.
    fn id(&self) -> &'static str;
    /// Appends this rule's findings to `out`.
    fn check(&self, graph: &Graph<'_>, out: &mut Vec<Issue>);
}

/// Runs a rule set over documents.
pub struct Validator {
    rules: Vec<Box<dyn Rule>>,
}

impl Validator {
    /// The built-in rule set.
    pub fn with_default_rules() -> Self {
        Self {
            rules: vec![
                Box::new(DuplicateIds),
                Box::new(NoStartEvent),
                Box::new(Disconnected),
                Box::new(GatewayNoOutgoing),
                Box::new(ExclusiveWithoutDefault),
                Box::new(WrongEndpointFlows),
                Box::new(DanglingFlowRefs),
                Box::new(BoundaryAttachment),
                Box::new(EventBasedTargets),
                Box::new(CrossContainerFlow),
                Box::new(SubprocessContents),
                Box::new(MessageFlowEndpoints),
            ],
        }
    }

    /// Validates `document`, most severe issues first.
    pub fn run(&self, document: &Document) -> Vec<Issue> {
        let graph = Graph::new(document);
        let mut issues = Vec::new();
        for rule in &self.rules {
            rule.check(&graph, &mut issues);
        }
        issues.sort_by_key(|issue| std::cmp::Reverse(issue.severity));
        issues
    }
}

fn label(element: &FlowElement, id: &BpmnId) -> String {
    element.name().map_or_else(
        || id.as_str().to_string(),
        |name| format!("\u{201c}{name}\u{201d}"),
    )
}

struct DuplicateIds;

impl Rule for DuplicateIds {
    fn id(&self) -> &'static str {
        "duplicate-id"
    }

    fn check(&self, graph: &Graph<'_>, out: &mut Vec<Issue>) {
        let mut seen: HashSet<&BpmnId> = HashSet::new();
        for element in graph.document.flow_elements() {
            if let Some(id) = element.id()
                && !seen.insert(id)
            {
                out.push(Issue {
                    rule: self.id(),
                    severity: Severity::Error,
                    element: Some(id.clone()),
                    message: format!("Duplicate id {id}"),
                });
            }
        }
    }
}

struct NoStartEvent;

impl Rule for NoStartEvent {
    fn id(&self) -> &'static str {
        "no-start-event"
    }

    fn check(&self, graph: &Graph<'_>, out: &mut Vec<Issue>) {
        for process in graph.document.processes() {
            let mut any_node = false;
            let mut any_start = false;
            for element in &process.flow_elements {
                match element {
                    FlowElement::Event(event) => {
                        any_node = true;
                        any_start |= matches!(event.position, EventPosition::Start { .. });
                    }
                    FlowElement::Activity(_) | FlowElement::Gateway(_) => any_node = true,
                    _ => {}
                }
            }
            if any_node && !any_start {
                out.push(Issue {
                    rule: self.id(),
                    severity: Severity::Warning,
                    element: None,
                    message: String::from("The process has no start event"),
                });
            }
        }
    }
}

struct Disconnected;

impl Rule for Disconnected {
    fn id(&self) -> &'static str {
        "disconnected"
    }

    fn check(&self, graph: &Graph<'_>, out: &mut Vec<Issue>) {
        for element in graph.document.flow_elements() {
            let Some(id) = element.id() else { continue };
            let is_node = match element {
                FlowElement::Event(event) => {
                    // A boundary event hangs off its host; no incoming needed.
                    !matches!(event.position, EventPosition::Boundary { .. })
                }
                // An event subprocess is started by its own events, never by
                // sequence flow; SubprocessContents polices its flows instead.
                FlowElement::Activity(activity) => !matches!(
                    &activity.kind,
                    ActivityKind::SubProcess(sub) if sub.kind == SubProcessKind::Event
                ),
                FlowElement::Gateway(_) => true,
                _ => false,
            };
            if is_node && graph.incoming(id).is_empty() && graph.outgoing(id).is_empty() {
                out.push(Issue {
                    rule: self.id(),
                    severity: Severity::Warning,
                    element: Some(id.clone()),
                    message: format!("{} is not connected to anything", label(element, id)),
                });
            }
        }
    }
}

struct GatewayNoOutgoing;

impl Rule for GatewayNoOutgoing {
    fn id(&self) -> &'static str {
        "gateway-no-outgoing"
    }

    fn check(&self, graph: &Graph<'_>, out: &mut Vec<Issue>) {
        for element in graph.document.flow_elements() {
            if let FlowElement::Gateway(gateway) = element {
                let id = &gateway.node.base.id;
                if graph.outgoing(id).is_empty() {
                    out.push(Issue {
                        rule: self.id(),
                        severity: Severity::Error,
                        element: Some(id.clone()),
                        message: format!("Gateway {} has no outgoing flow", label(element, id)),
                    });
                }
            }
        }
    }
}

struct ExclusiveWithoutDefault;

impl ExclusiveWithoutDefault {
    fn applies(gateway: &Gateway, graph: &Graph<'_>) -> bool {
        if gateway.kind != GatewayKind::Exclusive || gateway.default_flow.is_some() {
            return false;
        }
        let outgoing = graph.outgoing(&gateway.node.base.id);
        let unconditioned = outgoing
            .iter()
            .filter(|flow| {
                !matches!(
                    flow.condition,
                    Some(Condition::Expression { .. } | Condition::Raw(_))
                )
            })
            .count();
        outgoing.len() > 1 && unconditioned > 0
    }
}

impl Rule for ExclusiveWithoutDefault {
    fn id(&self) -> &'static str {
        "exclusive-without-default"
    }

    fn check(&self, graph: &Graph<'_>, out: &mut Vec<Issue>) {
        for element in graph.document.flow_elements() {
            if let FlowElement::Gateway(gateway) = element
                && Self::applies(gateway, graph)
            {
                let id = &gateway.node.base.id;
                out.push(Issue {
                    rule: self.id(),
                    severity: Severity::Warning,
                    element: Some(id.clone()),
                    message: format!(
                        "Exclusive gateway {} branches without conditions and has no default flow",
                        label(element, id)
                    ),
                });
            }
        }
    }
}

struct WrongEndpointFlows;

impl Rule for WrongEndpointFlows {
    fn id(&self) -> &'static str {
        "wrong-endpoint"
    }

    fn check(&self, graph: &Graph<'_>, out: &mut Vec<Issue>) {
        for element in graph.document.flow_elements() {
            if let FlowElement::Event(event) = element {
                let id = &event.node.base.id;
                match event.position {
                    EventPosition::Start { .. } if !graph.incoming(id).is_empty() => {
                        out.push(Issue {
                            rule: self.id(),
                            severity: Severity::Error,
                            element: Some(id.clone()),
                            message: format!(
                                "Start event {} cannot have incoming flows",
                                label(element, id)
                            ),
                        });
                    }
                    EventPosition::End if !graph.outgoing(id).is_empty() => {
                        out.push(Issue {
                            rule: self.id(),
                            severity: Severity::Error,
                            element: Some(id.clone()),
                            message: format!(
                                "End event {} cannot have outgoing flows",
                                label(element, id)
                            ),
                        });
                    }
                    _ => {}
                }
            }
        }
    }
}

struct DanglingFlowRefs;

impl Rule for DanglingFlowRefs {
    fn id(&self) -> &'static str {
        "dangling-flow"
    }

    fn check(&self, graph: &Graph<'_>, out: &mut Vec<Issue>) {
        for element in graph.document.flow_elements() {
            if let FlowElement::SequenceFlow(flow) = element {
                for (end, which) in [(&flow.source, "source"), (&flow.target, "target")] {
                    if !graph.index.contains_key(end) {
                        out.push(Issue {
                            rule: self.id(),
                            severity: Severity::Error,
                            element: Some(flow.base.id.clone()),
                            message: format!(
                                "Flow {} points at missing {which} {end}",
                                flow.base.id
                            ),
                        });
                    }
                }
            }
        }
    }
}

struct BoundaryAttachment;

impl Rule for BoundaryAttachment {
    fn id(&self) -> &'static str {
        "boundary-attachment"
    }

    fn check(&self, graph: &Graph<'_>, out: &mut Vec<Issue>) {
        for element in graph.document.flow_elements() {
            if let FlowElement::Event(event) = element
                && let EventPosition::Boundary { attached_to, .. } = &event.position
            {
                let id = &event.node.base.id;
                let host_is_activity =
                    matches!(graph.index.get(attached_to), Some(FlowElement::Activity(_)));
                if !host_is_activity {
                    out.push(Issue {
                        rule: self.id(),
                        severity: Severity::Error,
                        element: Some(id.clone()),
                        message: format!(
                            "Boundary event {} is not attached to an activity",
                            label(element, id)
                        ),
                    });
                }
            }
        }
    }
}

struct EventBasedTargets;

impl Rule for EventBasedTargets {
    fn id(&self) -> &'static str {
        "event-based-targets"
    }

    fn check(&self, graph: &Graph<'_>, out: &mut Vec<Issue>) {
        for element in graph.document.flow_elements() {
            let FlowElement::Gateway(gateway) = element else {
                continue;
            };
            if gateway.kind != GatewayKind::EventBased {
                continue;
            }
            let id = &gateway.node.base.id;
            for flow in graph.outgoing(id) {
                let ok = match graph.index.get(&flow.target) {
                    Some(FlowElement::Event(event)) => {
                        matches!(event.position, EventPosition::IntermediateCatch)
                    }
                    Some(FlowElement::Activity(activity)) => matches!(
                        activity.kind,
                        bpmn_core::model::ActivityKind::Task(TaskType::Receive)
                    ),
                    _ => false,
                };
                if !ok {
                    out.push(Issue {
                        rule: self.id(),
                        severity: Severity::Error,
                        element: Some(flow.target.clone()),
                        message: format!(
                            "Event-based gateway {} must lead to catch events or receive tasks",
                            label(element, id)
                        ),
                    });
                }
            }
        }
    }
}

struct CrossContainerFlow;

impl Rule for CrossContainerFlow {
    fn id(&self) -> &'static str {
        "cross-container-flow"
    }

    fn check(&self, graph: &Graph<'_>, out: &mut Vec<Issue>) {
        for element in graph.document.flow_elements() {
            let FlowElement::SequenceFlow(flow) = element else {
                continue;
            };
            let source = graph.document.container_of(&flow.source);
            let target = graph.document.container_of(&flow.target);
            if let (Some(source), Some(target)) = (source, target)
                && source != target
            {
                out.push(Issue {
                    rule: self.id(),
                    severity: Severity::Error,
                    element: Some(flow.base.id.clone()),
                    message: format!(
                        "Flow {} crosses a container boundary (use a message flow between pools, or connect through the subprocess itself)",
                        flow.base.id
                    ),
                });
            }
        }
    }
}

struct SubprocessContents;

impl Rule for SubprocessContents {
    fn id(&self) -> &'static str {
        "subprocess-contents"
    }

    fn check(&self, graph: &Graph<'_>, out: &mut Vec<Issue>) {
        for element in graph.document.flow_elements() {
            let FlowElement::Activity(activity) = element else {
                continue;
            };
            let ActivityKind::SubProcess(sub) = &activity.kind else {
                continue;
            };
            let id = &activity.node.base.id;

            let mut starts = sub.flow_elements.iter().filter_map(|child| match child {
                FlowElement::Event(event)
                    if matches!(event.position, EventPosition::Start { .. }) =>
                {
                    Some(event)
                }
                _ => None,
            });
            let first_start = starts.next();

            if sub.kind == SubProcessKind::Event {
                match first_start {
                    None => out.push(Issue {
                        rule: self.id(),
                        severity: Severity::Error,
                        element: Some(id.clone()),
                        message: format!(
                            "Event subprocess {} needs a start event that triggers it",
                            label(element, id)
                        ),
                    }),
                    Some(start) if start.definitions.is_empty() => out.push(Issue {
                        rule: self.id(),
                        severity: Severity::Warning,
                        element: Some(start.node.base.id.clone()),
                        message: format!(
                            "The start event of event subprocess {} should have a trigger (message, timer, error, ...)",
                            label(element, id)
                        ),
                    }),
                    Some(_) => {}
                }
                if !graph.incoming(id).is_empty() || !graph.outgoing(id).is_empty() {
                    out.push(Issue {
                        rule: self.id(),
                        severity: Severity::Error,
                        element: Some(id.clone()),
                        message: format!(
                            "Event subprocess {} cannot have incoming or outgoing flows",
                            label(element, id)
                        ),
                    });
                }
            } else if first_start.is_none() && !sub.flow_elements.is_empty() {
                out.push(Issue {
                    rule: self.id(),
                    severity: Severity::Warning,
                    element: Some(id.clone()),
                    message: format!("Subprocess {} has no start event", label(element, id)),
                });
            }
        }
    }
}

struct MessageFlowEndpoints;

impl Rule for MessageFlowEndpoints {
    fn id(&self) -> &'static str {
        "message-flow-endpoints"
    }

    fn check(&self, graph: &Graph<'_>, out: &mut Vec<Issue>) {
        let document = graph.document;
        // A message flow endpoint resolves to the pool it belongs to: the
        // backing process for nodes and participants, the participant itself
        // for black-box pools.
        let pool_of = |id: &BpmnId| -> Option<BpmnId> {
            if let Some(participant) = document.participant(id) {
                return Some(
                    participant
                        .process_ref
                        .clone()
                        .unwrap_or_else(|| participant.base.id.clone()),
                );
            }
            graph
                .index
                .contains_key(id)
                .then(|| document.process_of(id))?
        };

        for flow in document.message_flows() {
            let id = &flow.base.id;
            let source = pool_of(&flow.source);
            let target = pool_of(&flow.target);
            match (source, target) {
                (None, _) | (_, None) => out.push(Issue {
                    rule: self.id(),
                    severity: Severity::Error,
                    element: Some(id.clone()),
                    message: format!("Message flow {id} points at a missing element"),
                }),
                (Some(source), Some(target)) if source == target => out.push(Issue {
                    rule: self.id(),
                    severity: Severity::Error,
                    element: Some(id.clone()),
                    message: format!(
                        "Message flow {id} connects elements of the same pool (use a sequence flow)"
                    ),
                }),
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bpmn_core::NodeKind;
    use bpmn_core::geometry::Point;
    use bpmn_core::model::GatewayKind;

    fn validate(document: &Document) -> Vec<Issue> {
        Validator::with_default_rules().run(document)
    }

    fn rules(issues: &[Issue]) -> Vec<&'static str> {
        issues.iter().map(|i| i.rule).collect()
    }

    #[test]
    fn a_healthy_process_is_quiet() {
        let mut doc = Document::new();
        let start = doc
            .create_node(NodeKind::StartEvent, Point::new(0.0, 0.0))
            .unwrap();
        let task = doc
            .create_node(
                NodeKind::Task(bpmn_core::model::TaskType::User),
                Point::new(200.0, 0.0),
            )
            .unwrap();
        let end = doc
            .create_node(NodeKind::EndEvent, Point::new(400.0, 0.0))
            .unwrap();
        doc.connect(&start, &task).unwrap();
        doc.connect(&task, &end).unwrap();

        assert_eq!(validate(&doc), Vec::new());
    }

    #[test]
    fn finds_structural_problems() {
        let mut doc = Document::new();
        // Lone task: disconnected, and the process has no start event.
        let task = doc
            .create_node(
                NodeKind::Task(bpmn_core::model::TaskType::Generic),
                Point::new(0.0, 0.0),
            )
            .unwrap();
        // Gateway without outgoing flows.
        let gw = doc
            .create_node(
                NodeKind::Gateway(GatewayKind::Exclusive),
                Point::new(200.0, 0.0),
            )
            .unwrap();
        doc.connect(&task, &gw).unwrap();

        let issues = validate(&doc);
        let rules = rules(&issues);
        assert!(rules.contains(&"no-start-event"));
        assert!(rules.contains(&"gateway-no-outgoing"));
        assert!(!rules.contains(&"disconnected"), "task got connected");
        // Errors sort before warnings.
        assert_eq!(issues[0].severity, Severity::Error);
    }

    #[test]
    fn exclusive_gateway_branching_without_default_warns() {
        let mut doc = Document::new();
        let start = doc
            .create_node(NodeKind::StartEvent, Point::new(0.0, 0.0))
            .unwrap();
        let gw = doc
            .create_node(
                NodeKind::Gateway(GatewayKind::Exclusive),
                Point::new(200.0, 0.0),
            )
            .unwrap();
        let a = doc
            .create_node(NodeKind::EndEvent, Point::new(400.0, -50.0))
            .unwrap();
        let b = doc
            .create_node(NodeKind::EndEvent, Point::new(400.0, 50.0))
            .unwrap();
        doc.connect(&start, &gw).unwrap();
        let yes = doc.connect(&gw, &a).unwrap();
        doc.connect(&gw, &b).unwrap();

        assert!(rules(&validate(&doc)).contains(&"exclusive-without-default"));

        // A default flow silences it.
        assert!(doc.set_gateway_default(&gw, Some(yes)));
        assert!(!rules(&validate(&doc)).contains(&"exclusive-without-default"));
    }

    #[test]
    fn subprocess_rules() {
        use bpmn_core::geometry::Point;
        use bpmn_core::model::SubProcessKind;

        // An embedded subprocess with contents but no start event warns.
        let mut doc = Document::new();
        let start = doc
            .create_node(NodeKind::StartEvent, Point::new(-400.0, 0.0))
            .unwrap();
        let sub = doc
            .create_node(
                NodeKind::SubProcess(SubProcessKind::Embedded),
                Point::new(0.0, 0.0),
            )
            .unwrap();
        let task = doc
            .create_node(
                NodeKind::Task(bpmn_core::model::TaskType::User),
                Point::new(0.0, 0.0),
            )
            .unwrap();
        let end = doc
            .create_node(NodeKind::EndEvent, Point::new(400.0, 0.0))
            .unwrap();
        doc.connect(&start, &sub).unwrap();
        doc.connect(&sub, &end).unwrap();

        let issues = validate(&doc);
        assert!(rules(&issues).contains(&"subprocess-contents"));
        assert!(
            !rules(&issues).contains(&"cross-container-flow"),
            "outer flows are fine"
        );

        // A start event inside silences it (the inner pair is connected).
        let inner_start = doc
            .create_node(NodeKind::StartEvent, Point::new(-120.0, 0.0))
            .unwrap();
        doc.connect(&inner_start, &task).unwrap();
        assert!(!rules(&validate(&doc)).contains(&"subprocess-contents"));
    }

    #[test]
    fn event_subprocess_wants_a_triggered_start_and_no_flows() {
        use bpmn_core::geometry::Point;
        use bpmn_core::model::{EventDefinitionKind, SubProcessKind};

        let mut doc = Document::new();
        let sub = doc
            .create_node(
                NodeKind::SubProcess(SubProcessKind::Event),
                Point::new(0.0, 0.0),
            )
            .unwrap();
        // No start event at all: error.
        let issues = validate(&doc);
        assert!(
            issues
                .iter()
                .any(|i| i.rule == "subprocess-contents" && i.severity == Severity::Error)
        );

        // A plain start event: warning (it should carry a trigger)...
        let inner = doc
            .create_node(NodeKind::StartEvent, Point::new(0.0, 0.0))
            .unwrap();
        let issues = validate(&doc);
        assert!(
            issues
                .iter()
                .any(|i| i.rule == "subprocess-contents" && i.severity == Severity::Warning)
        );

        // ...and a typed one is quiet.
        doc.set_event_definition(&inner, Some(EventDefinitionKind::Error));
        assert!(!rules(&validate(&doc)).contains(&"subprocess-contents"));
        assert!(
            !validate(&doc)
                .iter()
                .any(|i| i.rule == "disconnected" && i.element.as_ref() == Some(&sub)),
            "the event subprocess itself is never wired by flows"
        );
    }

    #[test]
    fn message_flows_must_cross_pools() {
        use bpmn_core::geometry::Point;
        use bpmn_core::model::RootElement;

        let mut doc = Document::new();
        let a = doc
            .create_node(NodeKind::StartEvent, Point::new(0.0, 0.0))
            .unwrap();
        let b = doc
            .create_node(
                NodeKind::Task(bpmn_core::model::TaskType::Receive),
                Point::new(200.0, 0.0),
            )
            .unwrap();
        doc.create_pool(None, Point::new(0.0, 0.0)).unwrap();
        let pool_b = doc.create_pool(None, Point::new(0.0, 600.0)).unwrap();
        let c = doc
            .create_node(
                NodeKind::Task(bpmn_core::model::TaskType::Send),
                Point::new(0.0, 600.0),
            )
            .unwrap();

        // A proper cross-pool message flow is quiet.
        let message = doc.connect(&c, &b).unwrap();
        assert!(!rules(&validate(&doc)).contains(&"message-flow-endpoints"));

        // Deleting the target leaves... nothing; the flow dies with it. Force
        // a same-pool message flow instead by rewiring the model directly.
        let _ = message;
        if let Some(RootElement::Collaboration(collaboration)) = doc
            .definitions
            .root_elements
            .iter_mut()
            .find(|r| matches!(r, RootElement::Collaboration(_)))
        {
            collaboration.message_flows[0].source = a.clone();
        }
        assert!(rules(&validate(&doc)).contains(&"message-flow-endpoints"));

        let _ = pool_b;
    }

    #[test]
    fn boundary_needs_an_activity_host() {
        let mut doc = Document::new();
        let start = doc
            .create_node(NodeKind::StartEvent, Point::new(0.0, 0.0))
            .unwrap();
        let task = doc
            .create_node(
                NodeKind::Task(bpmn_core::model::TaskType::Service),
                Point::new(200.0, 0.0),
            )
            .unwrap();
        doc.connect(&start, &task).unwrap();
        let boundary = doc
            .create_boundary_event(&task, Point::new(220.0, 100.0))
            .unwrap();
        let end = doc
            .create_node(NodeKind::EndEvent, Point::new(400.0, 100.0))
            .unwrap();
        doc.connect(&boundary, &end).unwrap();

        assert_eq!(validate(&doc), Vec::new());

        // Deleting the host leaves... nothing: the boundary dies with it? No —
        // deletion only removes flows; the boundary stays attached to a ghost.
        let doomed: std::collections::HashSet<_> = [task].into_iter().collect();
        doc.delete_elements(&doomed);
        assert!(rules(&validate(&doc)).contains(&"boundary-attachment"));
    }
}
