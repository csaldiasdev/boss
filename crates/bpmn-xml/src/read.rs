//! The BPMN reader: recursive descent over `quick-xml` resolved events.
//!
//! Dispatch happens per container on `(namespace, local name)`. Anything the
//! model does not understand is captured verbatim by byte span — the start
//! position is recorded before each event is read, `read_to_end` skips to the
//! matching close tag, and the slice between the two positions becomes a
//! [`RawXml`] fragment.

use crate::error::ParseError;
use crate::ns;
use bpmn_core::di::{BpmnDiagram, BpmnEdge, BpmnLabel, BpmnPlane, BpmnShape, DiElement};
use bpmn_core::geometry::{Bounds, Point};
use bpmn_core::model::{
    Activity, ActivityKind, BaseElement, CallActivity, Collaboration, Condition, Definitions,
    Event as BpmnEvent, EventDefinition, EventDefinitionBody, EventDefinitionKind, EventPosition,
    FlowElement, FlowNode, Gateway, GatewayKind, Lane, LaneSet, LoopKind, LoopMarker, MessageFlow,
    Participant, Process, RootElement, SequenceFlow, SubProcess, SubProcessKind, TaskType,
};
use bpmn_core::{BpmnId, Document, RawXml};
use quick_xml::events::{BytesStart, Event};
use quick_xml::name::ResolveResult;
use quick_xml::reader::NsReader;

/// Parses a `.bpmn` document.
pub fn parse(input: &str) -> Result<Document, ParseError> {
    let mut cx = Cx {
        reader: NsReader::from_str(input),
        input,
    };

    loop {
        let (nsk, event) = cx.read()?;
        match event {
            Event::Start(e) if nsk == Ns::Model && local(&e) == "definitions" => {
                let definitions = parse_definitions(&mut cx, &e)?;
                return Ok(Document { definitions });
            }
            Event::Decl(_) | Event::Comment(_) | Event::PI(_) | Event::DocType(_) => {}
            Event::Text(t) if t.as_ref().trim().is_empty() => {}
            _ => return Err(ParseError::MissingDefinitions),
        }
    }
}

/// The namespaces the reader distinguishes, decoded eagerly so no borrow of
/// the reader outlives the event dispatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ns {
    Model,
    BpmnDi,
    Dc,
    Di,
    Other,
}

fn local<'a>(e: &'a BytesStart<'_>) -> &'a str {
    e.local_name().into_inner()
}

struct Cx<'i> {
    reader: NsReader<&'i [u8]>,
    input: &'i str,
}

impl<'i> Cx<'i> {
    fn pos(&self) -> usize {
        usize::try_from(self.reader.buffer_position()).unwrap_or(usize::MAX)
    }

    fn read(&mut self) -> Result<(Ns, Event<'i>), ParseError> {
        let position = self.reader.buffer_position();
        let (rr, event) = self
            .reader
            .read_resolved_event()
            .map_err(|source| ParseError::Xml { position, source })?;
        let nsk = match rr {
            ResolveResult::Bound(bound) => match bound.as_ref() {
                ns::MODEL => Ns::Model,
                ns::BPMNDI => Ns::BpmnDi,
                ns::DC => Ns::Dc,
                ns::DI => Ns::Di,
                _ => Ns::Other,
            },
            _ => Ns::Other,
        };
        Ok((nsk, event))
    }

    /// Captures the element whose `Start` event was just read, verbatim.
    fn capture(&mut self, start: usize, e: &BytesStart<'_>) -> Result<RawXml, ParseError> {
        let position = self.reader.buffer_position();
        self.reader
            .read_to_end(e.name())
            .map_err(|source| ParseError::Xml { position, source })?;
        Ok(RawXml(self.input[start..self.pos()].to_string()))
    }

    /// Captures a span that is already fully consumed (`Empty`, `Comment`).
    fn capture_span(&self, start: usize) -> RawXml {
        RawXml(self.input[start..self.pos()].to_string())
    }

    /// Captures either kind of element event, verbatim.
    fn capture_any(
        &mut self,
        start: usize,
        e: &BytesStart<'_>,
        empty: bool,
    ) -> Result<RawXml, ParseError> {
        if empty {
            Ok(self.capture_span(start))
        } else {
            self.capture(start, e)
        }
    }

    /// Reads the text content of the element whose `Start` was just read.
    fn text(&mut self, e: &BytesStart<'_>) -> Result<String, ParseError> {
        let position = self.reader.buffer_position();
        let raw = self
            .reader
            .read_text(e.name())
            .map_err(|source| ParseError::Xml { position, source })?;
        let text: &str = raw.as_ref();
        Ok(quick_xml::escape::unescape(text)
            .map_or_else(|_| text.to_string(), std::borrow::Cow::into_owned))
    }

    /// Consumes an element without keeping it (used only where the spec allows
    /// nothing we would need to preserve).
    fn skip(&mut self, e: &BytesStart<'_>) -> Result<(), ParseError> {
        let position = self.reader.buffer_position();
        self.reader
            .read_to_end(e.name())
            .map_err(|source| ParseError::Xml { position, source })?;
        Ok(())
    }
}

fn structure(msg: impl Into<String>) -> ParseError {
    ParseError::Structure(msg.into())
}

fn parse_bool(value: &str) -> Option<bool> {
    match value {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

/// One decoded attribute: the key exactly as written, plus the value.
struct Attr {
    key: String,
    value: String,
}

fn attrs(e: &BytesStart<'_>) -> Result<Vec<Attr>, ParseError> {
    let mut out = Vec::new();
    for attr in e.attributes() {
        let attr = attr.map_err(|e| structure(format!("bad attribute: {e}")))?;
        let key = attr.key.as_ref().to_string();
        let value = attr
            .normalized_value(quick_xml::XmlVersion::Implicit1_0)
            .map_err(|e| structure(format!("bad attribute value: {e}")))?
            .into_owned();
        out.push(Attr { key, value });
    }
    Ok(out)
}

/// Recognized attributes are taken out one by one; whatever remains becomes
/// the element's unknown attributes.
struct AttrSet {
    attrs: Vec<Attr>,
}

impl AttrSet {
    fn take(&mut self, key: &str) -> Option<String> {
        let index = self.attrs.iter().position(|a| a.key == key)?;
        Some(self.attrs.remove(index).value)
    }

    fn take_id(&mut self, key: &str) -> Option<BpmnId> {
        self.take(key).map(|v| BpmnId::new(v.as_str()))
    }

    fn take_bool(&mut self, key: &str) -> Option<bool> {
        self.take(key).and_then(|v| parse_bool(&v))
    }

    fn take_f64(&mut self, key: &str, what: &str) -> Result<f64, ParseError> {
        let value = self
            .take(key)
            .ok_or_else(|| structure(format!("missing {key:?} in {what}")))?;
        value
            .parse()
            .map_err(|_| structure(format!("invalid number {value:?} in {what}")))
    }

    fn is_empty(&self) -> bool {
        self.attrs.is_empty()
    }

    fn rest(self) -> Vec<(String, String)> {
        self.attrs.into_iter().map(|a| (a.key, a.value)).collect()
    }
}

fn attr_set(e: &BytesStart<'_>) -> Result<AttrSet, ParseError> {
    Ok(AttrSet { attrs: attrs(e)? })
}

// ---------------------------------------------------------------------------
// definitions
// ---------------------------------------------------------------------------

fn parse_definitions(cx: &mut Cx<'_>, start: &BytesStart<'_>) -> Result<Definitions, ParseError> {
    let mut def = Definitions::default();

    for attr in attrs(start)? {
        if attr.key == "xmlns" {
            def.ns_decls.push((None, attr.value));
        } else if let Some(prefix) = attr.key.strip_prefix("xmlns:") {
            def.ns_decls.push((Some(prefix.to_string()), attr.value));
        } else {
            match attr.key.as_str() {
                "id" => def.id = Some(BpmnId::new(attr.value.as_str())),
                "targetNamespace" => def.target_namespace = attr.value,
                "exporter" => def.exporter = Some(attr.value),
                "exporterVersion" => def.exporter_version = Some(attr.value),
                _ => def.unknown_attrs.push((attr.key, attr.value)),
            }
        }
    }

    loop {
        let pos = cx.pos();
        let (nsk, event) = cx.read()?;
        let (e, empty) = match event {
            Event::Start(e) => (e, false),
            Event::Empty(e) => (e, true),
            Event::Comment(_) => {
                def.root_elements
                    .push(RootElement::Unknown(cx.capture_span(pos)));
                continue;
            }
            Event::End(_) => return Ok(def),
            Event::Eof => return Err(structure("unexpected end of file in <definitions>")),
            _ => continue,
        };

        match (nsk, local(&e)) {
            (Ns::Model, "process") => def
                .root_elements
                .push(RootElement::Process(parse_process(cx, &e, empty)?)),
            (Ns::Model, "collaboration") => {
                def.root_elements
                    .push(RootElement::Collaboration(parse_collaboration(
                        cx, &e, empty,
                    )?));
            }
            (Ns::BpmnDi, "BPMNDiagram") => def.diagrams.push(parse_diagram(cx, &e, empty)?),
            _ => def
                .root_elements
                .push(RootElement::Unknown(cx.capture_any(pos, &e, empty)?)),
        }
    }
}

// ---------------------------------------------------------------------------
// process and flow elements
// ---------------------------------------------------------------------------

/// Handles children shared by every `BaseElement`: `documentation` and
/// `extensionElements`. Returns whether the event was consumed.
fn base_child(
    cx: &mut Cx<'_>,
    base: &mut BaseElement,
    pos: usize,
    nsk: Ns,
    e: &BytesStart<'_>,
    empty: bool,
) -> Result<bool, ParseError> {
    if nsk != Ns::Model {
        return Ok(false);
    }
    match local(e) {
        "documentation" => {
            let text = if empty { String::new() } else { cx.text(e)? };
            base.documentation.push(text);
            Ok(true)
        }
        "extensionElements" => {
            base.extension_elements = Some(cx.capture_any(pos, e, empty)?);
            Ok(true)
        }
        _ => Ok(false),
    }
}

/// Handles children shared by every flow node: base children plus
/// `incoming`/`outgoing` references.
fn node_child(
    cx: &mut Cx<'_>,
    node: &mut FlowNode,
    pos: usize,
    nsk: Ns,
    e: &BytesStart<'_>,
    empty: bool,
) -> Result<bool, ParseError> {
    if base_child(cx, &mut node.base, pos, nsk, e, empty)? {
        return Ok(true);
    }
    if nsk != Ns::Model {
        return Ok(false);
    }
    match local(e) {
        "incoming" => {
            if !empty {
                node.incoming.push(BpmnId::new(cx.text(e)?.trim()));
            }
            Ok(true)
        }
        "outgoing" => {
            if !empty {
                node.outgoing.push(BpmnId::new(cx.text(e)?.trim()));
            }
            Ok(true)
        }
        _ => Ok(false),
    }
}

fn flow_node_from(set: &mut AttrSet) -> FlowNode {
    FlowNode {
        base: BaseElement::new(set.take_id("id").unwrap_or_default()),
        name: set.take("name"),
        incoming: Vec::new(),
        outgoing: Vec::new(),
    }
}

fn parse_process(
    cx: &mut Cx<'_>,
    start: &BytesStart<'_>,
    empty: bool,
) -> Result<Process, ParseError> {
    let mut set = attr_set(start)?;
    let mut process = Process {
        base: BaseElement::new(set.take_id("id").unwrap_or_default()),
        name: set.take("name"),
        is_executable: set.take_bool("isExecutable"),
        ..Process::default()
    };
    process.base.unknown_attrs = set.rest();

    if empty {
        return Ok(process);
    }

    loop {
        let pos = cx.pos();
        let (nsk, event) = cx.read()?;
        let (e, is_empty) = match event {
            Event::Start(e) => (e, false),
            Event::Empty(e) => (e, true),
            Event::Comment(_) => {
                process.base.unknown_children.push(cx.capture_span(pos));
                continue;
            }
            Event::End(_) => return Ok(process),
            Event::Eof => return Err(structure("unexpected end of file in <process>")),
            _ => continue,
        };

        if base_child(cx, &mut process.base, pos, nsk, &e, is_empty)? {
            continue;
        }
        if nsk == Ns::Model && local(&e) == "laneSet" {
            process.lane_sets.push(parse_lane_set(cx, &e, is_empty)?);
            continue;
        }

        let element = match parse_flow_element(cx, nsk, &e, is_empty)? {
            Some(element) => element,
            None => FlowElement::Unknown(cx.capture_any(pos, &e, is_empty)?),
        };
        process.flow_elements.push(element);
    }
}

fn parse_lane_set(
    cx: &mut Cx<'_>,
    start: &BytesStart<'_>,
    empty: bool,
) -> Result<LaneSet, ParseError> {
    let mut set = attr_set(start)?;
    let mut lane_set = LaneSet {
        id: set.take_id("id"),
        ..LaneSet::default()
    };
    lane_set.unknown_attrs = set.rest();

    if empty {
        return Ok(lane_set);
    }

    loop {
        let pos = cx.pos();
        let (nsk, event) = cx.read()?;
        let (e, is_empty) = match event {
            Event::Start(e) => (e, false),
            Event::Empty(e) => (e, true),
            Event::End(_) => return Ok(lane_set),
            Event::Eof => return Err(structure("unexpected end of file in laneSet")),
            _ => continue,
        };

        if nsk == Ns::Model && local(&e) == "lane" {
            lane_set.lanes.push(parse_lane(cx, &e, is_empty)?);
        } else {
            // A lane set holds only lanes; anything else is consumed into the
            // span so the parse keeps its position (nothing known writes it).
            let _ = cx.capture_any(pos, &e, is_empty)?;
        }
    }
}

fn parse_lane(cx: &mut Cx<'_>, start: &BytesStart<'_>, empty: bool) -> Result<Lane, ParseError> {
    let mut set = attr_set(start)?;
    let mut lane = Lane {
        base: BaseElement::new(set.take_id("id").unwrap_or_default()),
        name: set.take("name"),
        ..Lane::default()
    };
    lane.base.unknown_attrs = set.rest();

    if empty {
        return Ok(lane);
    }

    loop {
        let pos = cx.pos();
        let (nsk, event) = cx.read()?;
        let (e, is_empty) = match event {
            Event::Start(e) => (e, false),
            Event::Empty(e) => (e, true),
            Event::Comment(_) => {
                lane.base.unknown_children.push(cx.capture_span(pos));
                continue;
            }
            Event::End(_) => return Ok(lane),
            Event::Eof => return Err(structure("unexpected end of file in lane")),
            _ => continue,
        };

        if base_child(cx, &mut lane.base, pos, nsk, &e, is_empty)? {
            continue;
        }
        match (nsk, local(&e)) {
            (Ns::Model, "flowNodeRef") => {
                if !is_empty {
                    lane.flow_node_refs.push(BpmnId::new(cx.text(&e)?.trim()));
                }
            }
            (Ns::Model, "childLaneSet") => {
                lane.child_lane_set = Some(parse_lane_set(cx, &e, is_empty)?);
            }
            _ => lane
                .base
                .unknown_children
                .push(cx.capture_any(pos, &e, is_empty)?),
        }
    }
}

fn parse_collaboration(
    cx: &mut Cx<'_>,
    start: &BytesStart<'_>,
    empty: bool,
) -> Result<Collaboration, ParseError> {
    let mut set = attr_set(start)?;
    let mut collaboration = Collaboration {
        base: BaseElement::new(set.take_id("id").unwrap_or_default()),
        name: set.take("name"),
        ..Collaboration::default()
    };
    collaboration.base.unknown_attrs = set.rest();

    if empty {
        return Ok(collaboration);
    }

    loop {
        let pos = cx.pos();
        let (nsk, event) = cx.read()?;
        let (e, is_empty) = match event {
            Event::Start(e) => (e, false),
            Event::Empty(e) => (e, true),
            Event::Comment(_) => {
                collaboration
                    .base
                    .unknown_children
                    .push(cx.capture_span(pos));
                continue;
            }
            Event::End(_) => return Ok(collaboration),
            Event::Eof => return Err(structure("unexpected end of file in collaboration")),
            _ => continue,
        };

        if base_child(cx, &mut collaboration.base, pos, nsk, &e, is_empty)? {
            continue;
        }
        match (nsk, local(&e)) {
            (Ns::Model, "participant") => {
                collaboration
                    .participants
                    .push(parse_participant(cx, &e, is_empty)?);
            }
            (Ns::Model, "messageFlow") => {
                let mut set = attr_set(&e)?;
                let mut flow = MessageFlow {
                    base: BaseElement::new(set.take_id("id").unwrap_or_default()),
                    name: set.take("name"),
                    source: set.take_id("sourceRef").unwrap_or_default(),
                    target: set.take_id("targetRef").unwrap_or_default(),
                };
                flow.base.unknown_attrs = set.rest();
                if !is_empty {
                    parse_base_only(cx, &mut flow.base, "messageFlow")?;
                }
                collaboration.message_flows.push(flow);
            }
            _ => collaboration
                .base
                .unknown_children
                .push(cx.capture_any(pos, &e, is_empty)?),
        }
    }
}

fn parse_participant(
    cx: &mut Cx<'_>,
    start: &BytesStart<'_>,
    empty: bool,
) -> Result<Participant, ParseError> {
    let mut set = attr_set(start)?;
    let mut participant = Participant {
        base: BaseElement::new(set.take_id("id").unwrap_or_default()),
        name: set.take("name"),
        process_ref: set.take_id("processRef"),
    };
    participant.base.unknown_attrs = set.rest();
    if !empty {
        parse_base_only(cx, &mut participant.base, "participant")?;
    }
    Ok(participant)
}

/// Consumes an element whose only modeled children are the base ones
/// (`documentation`, `extensionElements`); the rest is preserved verbatim.
fn parse_base_only(cx: &mut Cx<'_>, base: &mut BaseElement, what: &str) -> Result<(), ParseError> {
    loop {
        let pos = cx.pos();
        let (nsk, event) = cx.read()?;
        let (e, is_empty) = match event {
            Event::Start(e) => (e, false),
            Event::Empty(e) => (e, true),
            Event::Comment(_) => {
                base.unknown_children.push(cx.capture_span(pos));
                continue;
            }
            Event::End(_) => return Ok(()),
            Event::Eof => return Err(structure(format!("unexpected end of file in {what}"))),
            _ => continue,
        };
        if !base_child(cx, base, pos, nsk, &e, is_empty)? {
            base.unknown_children
                .push(cx.capture_any(pos, &e, is_empty)?);
        }
    }
}

/// Parses one flow element when the tag is modeled; `None` asks the caller to
/// capture the element verbatim. Shared by processes and subprocesses.
fn parse_flow_element(
    cx: &mut Cx<'_>,
    nsk: Ns,
    e: &BytesStart<'_>,
    is_empty: bool,
) -> Result<Option<FlowElement>, ParseError> {
    if nsk != Ns::Model {
        return Ok(None);
    }
    Ok(Some(match local(e) {
        "startEvent"
        | "endEvent"
        | "intermediateCatchEvent"
        | "intermediateThrowEvent"
        | "boundaryEvent" => FlowElement::Event(parse_event(cx, e, is_empty)?),
        "task" => parse_task(cx, e, is_empty, TaskType::Generic)?,
        "userTask" => parse_task(cx, e, is_empty, TaskType::User)?,
        "serviceTask" => parse_task(cx, e, is_empty, TaskType::Service)?,
        "scriptTask" => parse_task(cx, e, is_empty, TaskType::Script)?,
        "manualTask" => parse_task(cx, e, is_empty, TaskType::Manual)?,
        "sendTask" => parse_task(cx, e, is_empty, TaskType::Send)?,
        "receiveTask" => parse_task(cx, e, is_empty, TaskType::Receive)?,
        "businessRuleTask" => parse_task(cx, e, is_empty, TaskType::BusinessRule)?,
        // Ad-hoc subprocesses are not modeled yet; they fall through to the
        // verbatim capture like any other unknown element.
        "subProcess" | "transaction" => parse_sub_process(cx, e, is_empty)?,
        "callActivity" => parse_call_activity(cx, e, is_empty)?,
        "exclusiveGateway" => parse_gateway(cx, e, is_empty, GatewayKind::Exclusive)?,
        "parallelGateway" => parse_gateway(cx, e, is_empty, GatewayKind::Parallel)?,
        "inclusiveGateway" => parse_gateway(cx, e, is_empty, GatewayKind::Inclusive)?,
        "eventBasedGateway" => parse_gateway(cx, e, is_empty, GatewayKind::EventBased)?,
        "complexGateway" => parse_gateway(cx, e, is_empty, GatewayKind::Complex)?,
        "sequenceFlow" => FlowElement::SequenceFlow(parse_sequence_flow(cx, e, is_empty)?),
        _ => return Ok(None),
    }))
}

fn parse_event(
    cx: &mut Cx<'_>,
    start: &BytesStart<'_>,
    empty: bool,
) -> Result<BpmnEvent, ParseError> {
    let tag = local(start).to_string();
    let mut set = attr_set(start)?;
    let mut node = flow_node_from(&mut set);

    let position = match tag.as_str() {
        "startEvent" => EventPosition::Start {
            is_interrupting: set.take_bool("isInterrupting"),
        },
        "endEvent" => EventPosition::End,
        "intermediateCatchEvent" => EventPosition::IntermediateCatch,
        "intermediateThrowEvent" => EventPosition::IntermediateThrow,
        "boundaryEvent" => EventPosition::Boundary {
            attached_to: set.take_id("attachedToRef").unwrap_or_default(),
            cancel_activity: set.take_bool("cancelActivity"),
        },
        other => return Err(structure(format!("unexpected event tag {other:?}"))),
    };
    node.base.unknown_attrs = set.rest();

    let mut event = BpmnEvent {
        node,
        position,
        definitions: Vec::new(),
    };

    if empty {
        return Ok(event);
    }

    loop {
        let pos = cx.pos();
        let (nsk, ev) = cx.read()?;
        let (e, is_empty) = match ev {
            Event::Start(e) => (e, false),
            Event::Empty(e) => (e, true),
            Event::Comment(_) => {
                event.node.base.unknown_children.push(cx.capture_span(pos));
                continue;
            }
            Event::End(_) => return Ok(event),
            Event::Eof => return Err(structure("unexpected end of file in event")),
            _ => continue,
        };

        if node_child(cx, &mut event.node, pos, nsk, &e, is_empty)? {
            continue;
        }

        if nsk == Ns::Model && local(&e).ends_with("EventDefinition") {
            event
                .definitions
                .push(parse_event_definition(cx, pos, &e, is_empty)?);
        } else {
            event
                .node
                .base
                .unknown_children
                .push(cx.capture_any(pos, &e, is_empty)?);
        }
    }
}

/// The trigger kind is always taken from the tag; the body is structured only
/// when the element carries nothing we would lose, and verbatim otherwise.
fn parse_event_definition(
    cx: &mut Cx<'_>,
    pos: usize,
    e: &BytesStart<'_>,
    empty: bool,
) -> Result<EventDefinition, ParseError> {
    let kind = match local(e) {
        "messageEventDefinition" => EventDefinitionKind::Message,
        "timerEventDefinition" => EventDefinitionKind::Timer,
        "signalEventDefinition" => EventDefinitionKind::Signal,
        "errorEventDefinition" => EventDefinitionKind::Error,
        "escalationEventDefinition" => EventDefinitionKind::Escalation,
        "conditionalEventDefinition" => EventDefinitionKind::Conditional,
        "compensateEventDefinition" => EventDefinitionKind::Compensate,
        "linkEventDefinition" => EventDefinitionKind::Link,
        "cancelEventDefinition" => EventDefinitionKind::Cancel,
        "terminateEventDefinition" => EventDefinitionKind::Terminate,
        _ => EventDefinitionKind::Other,
    };

    let body = if empty && kind != EventDefinitionKind::Other {
        let mut set = attr_set(e)?;
        let id = set.take_id("id");
        let message_ref = (kind == EventDefinitionKind::Message)
            .then(|| set.take("messageRef"))
            .flatten();
        if set.is_empty() {
            let _ = cx.capture_span(pos);
            if kind == EventDefinitionKind::Message {
                EventDefinitionBody::Message { id, message_ref }
            } else {
                EventDefinitionBody::Empty { id }
            }
        } else {
            EventDefinitionBody::Raw(cx.capture_span(pos))
        }
    } else {
        EventDefinitionBody::Raw(cx.capture_any(pos, e, empty)?)
    };

    Ok(EventDefinition { kind, body })
}

fn parse_task(
    cx: &mut Cx<'_>,
    start: &BytesStart<'_>,
    empty: bool,
    task_type: TaskType,
) -> Result<FlowElement, ParseError> {
    let mut set = attr_set(start)?;
    let mut node = flow_node_from(&mut set);
    node.base.unknown_attrs = set.rest();

    let mut activity = Activity {
        node,
        kind: ActivityKind::Task(task_type),
        loop_marker: None,
    };

    if !empty {
        loop {
            let pos = cx.pos();
            let (nsk, ev) = cx.read()?;
            let (e, is_empty) = match ev {
                Event::Start(e) => (e, false),
                Event::Empty(e) => (e, true),
                Event::Comment(_) => {
                    activity
                        .node
                        .base
                        .unknown_children
                        .push(cx.capture_span(pos));
                    continue;
                }
                Event::End(_) => break,
                Event::Eof => return Err(structure("unexpected end of file in task")),
                _ => continue,
            };

            if node_child(cx, &mut activity.node, pos, nsk, &e, is_empty)? {
                continue;
            }

            if nsk == Ns::Model
                && matches!(
                    local(&e),
                    "standardLoopCharacteristics" | "multiInstanceLoopCharacteristics"
                )
            {
                activity.loop_marker = Some(parse_loop_marker(cx, pos, &e, is_empty)?);
            } else {
                activity
                    .node
                    .base
                    .unknown_children
                    .push(cx.capture_any(pos, &e, is_empty)?);
            }
        }
    }

    Ok(FlowElement::Activity(activity))
}

/// Parses a `subProcess` or `transaction`: an activity whose children mix
/// nested flow elements with the usual node children.
fn parse_sub_process(
    cx: &mut Cx<'_>,
    start: &BytesStart<'_>,
    empty: bool,
) -> Result<FlowElement, ParseError> {
    let is_transaction = local(start) == "transaction";
    let mut set = attr_set(start)?;
    let mut node = flow_node_from(&mut set);
    let triggered_by_event = set.take_bool("triggeredByEvent").unwrap_or(false);
    node.base.unknown_attrs = set.rest();

    let kind = if is_transaction {
        SubProcessKind::Transaction
    } else if triggered_by_event {
        SubProcessKind::Event
    } else {
        SubProcessKind::Embedded
    };

    let mut activity = Activity {
        node,
        kind: ActivityKind::SubProcess(SubProcess {
            kind,
            flow_elements: Vec::new(),
        }),
        loop_marker: None,
    };

    if !empty {
        loop {
            let pos = cx.pos();
            let (nsk, ev) = cx.read()?;
            let (e, is_empty) = match ev {
                Event::Start(e) => (e, false),
                Event::Empty(e) => (e, true),
                Event::Comment(_) => {
                    push_sub_element(&mut activity, FlowElement::Unknown(cx.capture_span(pos)));
                    continue;
                }
                Event::End(_) => break,
                Event::Eof => return Err(structure("unexpected end of file in subProcess")),
                _ => continue,
            };

            if node_child(cx, &mut activity.node, pos, nsk, &e, is_empty)? {
                continue;
            }

            if nsk == Ns::Model
                && matches!(
                    local(&e),
                    "standardLoopCharacteristics" | "multiInstanceLoopCharacteristics"
                )
            {
                activity.loop_marker = Some(parse_loop_marker(cx, pos, &e, is_empty)?);
                continue;
            }

            // Children keep document order: unknown elements interleave with
            // the modeled ones, exactly like a process body.
            let element = match parse_flow_element(cx, nsk, &e, is_empty)? {
                Some(element) => element,
                None => FlowElement::Unknown(cx.capture_any(pos, &e, is_empty)?),
            };
            push_sub_element(&mut activity, element);
        }
    }

    Ok(FlowElement::Activity(activity))
}

fn push_sub_element(activity: &mut Activity, element: FlowElement) {
    if let ActivityKind::SubProcess(sub) = &mut activity.kind {
        sub.flow_elements.push(element);
    }
}

fn parse_call_activity(
    cx: &mut Cx<'_>,
    start: &BytesStart<'_>,
    empty: bool,
) -> Result<FlowElement, ParseError> {
    let mut set = attr_set(start)?;
    let mut node = flow_node_from(&mut set);
    let called_element = set.take("calledElement");
    node.base.unknown_attrs = set.rest();

    let mut activity = Activity {
        node,
        kind: ActivityKind::CallActivity(CallActivity { called_element }),
        loop_marker: None,
    };

    if !empty {
        loop {
            let pos = cx.pos();
            let (nsk, ev) = cx.read()?;
            let (e, is_empty) = match ev {
                Event::Start(e) => (e, false),
                Event::Empty(e) => (e, true),
                Event::Comment(_) => {
                    activity
                        .node
                        .base
                        .unknown_children
                        .push(cx.capture_span(pos));
                    continue;
                }
                Event::End(_) => break,
                Event::Eof => return Err(structure("unexpected end of file in callActivity")),
                _ => continue,
            };

            if node_child(cx, &mut activity.node, pos, nsk, &e, is_empty)? {
                continue;
            }

            if nsk == Ns::Model
                && matches!(
                    local(&e),
                    "standardLoopCharacteristics" | "multiInstanceLoopCharacteristics"
                )
            {
                activity.loop_marker = Some(parse_loop_marker(cx, pos, &e, is_empty)?);
            } else {
                activity
                    .node
                    .base
                    .unknown_children
                    .push(cx.capture_any(pos, &e, is_empty)?);
            }
        }
    }

    Ok(FlowElement::Activity(activity))
}

fn parse_gateway(
    cx: &mut Cx<'_>,
    start: &BytesStart<'_>,
    empty: bool,
    kind: GatewayKind,
) -> Result<FlowElement, ParseError> {
    let mut set = attr_set(start)?;
    let mut node = flow_node_from(&mut set);
    let default_flow = set.take_id("default");
    let instantiate = if kind == GatewayKind::EventBased {
        set.take_bool("instantiate")
    } else {
        None
    };
    node.base.unknown_attrs = set.rest();

    let mut gateway = Gateway {
        node,
        kind,
        default_flow,
        instantiate,
    };

    if !empty {
        loop {
            let pos = cx.pos();
            let (nsk, ev) = cx.read()?;
            let (e, is_empty) = match ev {
                Event::Start(e) => (e, false),
                Event::Empty(e) => (e, true),
                Event::Comment(_) => {
                    gateway
                        .node
                        .base
                        .unknown_children
                        .push(cx.capture_span(pos));
                    continue;
                }
                Event::End(_) => break,
                Event::Eof => return Err(structure("unexpected end of file in gateway")),
                _ => continue,
            };

            if !node_child(cx, &mut gateway.node, pos, nsk, &e, is_empty)? {
                gateway
                    .node
                    .base
                    .unknown_children
                    .push(cx.capture_any(pos, &e, is_empty)?);
            }
        }
    }

    Ok(FlowElement::Gateway(gateway))
}

fn parse_sequence_flow(
    cx: &mut Cx<'_>,
    start: &BytesStart<'_>,
    empty: bool,
) -> Result<SequenceFlow, ParseError> {
    let mut set = attr_set(start)?;
    let mut flow = SequenceFlow {
        base: BaseElement::new(set.take_id("id").unwrap_or_default()),
        name: set.take("name"),
        source: set.take_id("sourceRef").unwrap_or_default(),
        target: set.take_id("targetRef").unwrap_or_default(),
        condition: None,
    };
    flow.base.unknown_attrs = set.rest();

    if empty {
        return Ok(flow);
    }

    loop {
        let pos = cx.pos();
        let (nsk, ev) = cx.read()?;
        let (e, is_empty) = match ev {
            Event::Start(e) => (e, false),
            Event::Empty(e) => (e, true),
            Event::Comment(_) => {
                flow.base.unknown_children.push(cx.capture_span(pos));
                continue;
            }
            Event::End(_) => return Ok(flow),
            Event::Eof => return Err(structure("unexpected end of file in sequenceFlow")),
            _ => continue,
        };

        if base_child(cx, &mut flow.base, pos, nsk, &e, is_empty)? {
            continue;
        }

        if nsk == Ns::Model && local(&e) == "conditionExpression" {
            flow.condition = Some(parse_condition(cx, pos, &e, is_empty)?);
        } else {
            flow.base
                .unknown_children
                .push(cx.capture_any(pos, &e, is_empty)?);
        }
    }
}

// ---------------------------------------------------------------------------
// BPMN DI
// ---------------------------------------------------------------------------

fn parse_diagram(
    cx: &mut Cx<'_>,
    start: &BytesStart<'_>,
    empty: bool,
) -> Result<BpmnDiagram, ParseError> {
    let mut set = attr_set(start)?;
    let mut diagram = BpmnDiagram {
        id: set.take_id("id"),
        name: set.take("name"),
        ..BpmnDiagram::default()
    };
    diagram.unknown_attrs = set.rest();

    if empty {
        return Ok(diagram);
    }

    loop {
        let pos = cx.pos();
        let (nsk, ev) = cx.read()?;
        let (e, is_empty) = match ev {
            Event::Start(e) => (e, false),
            Event::Empty(e) => (e, true),
            Event::Comment(_) => {
                diagram.unknown_children.push(cx.capture_span(pos));
                continue;
            }
            Event::End(_) => return Ok(diagram),
            Event::Eof => return Err(structure("unexpected end of file in BPMNDiagram")),
            _ => continue,
        };

        if nsk == Ns::BpmnDi && local(&e) == "BPMNPlane" {
            diagram.plane = Some(parse_plane(cx, &e, is_empty)?);
        } else {
            diagram
                .unknown_children
                .push(cx.capture_any(pos, &e, is_empty)?);
        }
    }
}

fn parse_plane(
    cx: &mut Cx<'_>,
    start: &BytesStart<'_>,
    empty: bool,
) -> Result<BpmnPlane, ParseError> {
    let mut set = attr_set(start)?;
    let mut plane = BpmnPlane {
        id: set.take_id("id"),
        bpmn_element: set.take_id("bpmnElement"),
        ..BpmnPlane::default()
    };
    plane.unknown_attrs = set.rest();

    if empty {
        return Ok(plane);
    }

    loop {
        let pos = cx.pos();
        let (nsk, ev) = cx.read()?;
        let (e, is_empty) = match ev {
            Event::Start(e) => (e, false),
            Event::Empty(e) => (e, true),
            Event::Comment(_) => {
                plane
                    .elements
                    .push(DiElement::Unknown(cx.capture_span(pos)));
                continue;
            }
            Event::End(_) => return Ok(plane),
            Event::Eof => return Err(structure("unexpected end of file in BPMNPlane")),
            _ => continue,
        };

        match (nsk, local(&e)) {
            (Ns::BpmnDi, "BPMNShape") => plane
                .elements
                .push(DiElement::Shape(parse_shape(cx, &e, is_empty)?)),
            (Ns::BpmnDi, "BPMNEdge") => plane
                .elements
                .push(DiElement::Edge(parse_edge(cx, &e, is_empty)?)),
            _ => plane
                .elements
                .push(DiElement::Unknown(cx.capture_any(pos, &e, is_empty)?)),
        }
    }
}

fn parse_bounds(e: &BytesStart<'_>) -> Result<Bounds, ParseError> {
    let mut set = attr_set(e)?;
    Ok(Bounds::new(
        set.take_f64("x", "dc:Bounds")?,
        set.take_f64("y", "dc:Bounds")?,
        set.take_f64("width", "dc:Bounds")?,
        set.take_f64("height", "dc:Bounds")?,
    ))
}

fn parse_shape(
    cx: &mut Cx<'_>,
    start: &BytesStart<'_>,
    empty: bool,
) -> Result<BpmnShape, ParseError> {
    let mut set = attr_set(start)?;
    let mut shape = BpmnShape {
        id: set.take_id("id"),
        bpmn_element: set.take_id("bpmnElement").unwrap_or_default(),
        is_marker_visible: set.take_bool("isMarkerVisible"),
        is_expanded: set.take_bool("isExpanded"),
        is_horizontal: set.take_bool("isHorizontal"),
        ..BpmnShape::default()
    };
    shape.unknown_attrs = set.rest();

    if empty {
        return Ok(shape);
    }

    loop {
        let pos = cx.pos();
        let (nsk, ev) = cx.read()?;
        let (e, is_empty) = match ev {
            Event::Start(e) => (e, false),
            Event::Empty(e) => (e, true),
            Event::Comment(_) => {
                shape.unknown_children.push(cx.capture_span(pos));
                continue;
            }
            Event::End(_) => return Ok(shape),
            Event::Eof => return Err(structure("unexpected end of file in BPMNShape")),
            _ => continue,
        };

        match (nsk, local(&e)) {
            (Ns::Dc, "Bounds") => {
                shape.bounds = parse_bounds(&e)?;
                if !is_empty {
                    cx.skip(&e)?;
                }
            }
            (Ns::BpmnDi, "BPMNLabel") => shape.label = Some(parse_label(cx, &e, is_empty)?),
            _ => shape
                .unknown_children
                .push(cx.capture_any(pos, &e, is_empty)?),
        }
    }
}

fn parse_edge(
    cx: &mut Cx<'_>,
    start: &BytesStart<'_>,
    empty: bool,
) -> Result<BpmnEdge, ParseError> {
    let mut set = attr_set(start)?;
    let mut edge = BpmnEdge {
        id: set.take_id("id"),
        bpmn_element: set.take_id("bpmnElement").unwrap_or_default(),
        ..BpmnEdge::default()
    };
    edge.unknown_attrs = set.rest();

    if empty {
        return Ok(edge);
    }

    loop {
        let pos = cx.pos();
        let (nsk, ev) = cx.read()?;
        let (e, is_empty) = match ev {
            Event::Start(e) => (e, false),
            Event::Empty(e) => (e, true),
            Event::Comment(_) => {
                edge.unknown_children.push(cx.capture_span(pos));
                continue;
            }
            Event::End(_) => return Ok(edge),
            Event::Eof => return Err(structure("unexpected end of file in BPMNEdge")),
            _ => continue,
        };

        match (nsk, local(&e)) {
            (Ns::Di, "waypoint") => {
                let mut set = attr_set(&e)?;
                edge.waypoints.push(Point::new(
                    set.take_f64("x", "di:waypoint")?,
                    set.take_f64("y", "di:waypoint")?,
                ));
                if !is_empty {
                    cx.skip(&e)?;
                }
            }
            (Ns::BpmnDi, "BPMNLabel") => edge.label = Some(parse_label(cx, &e, is_empty)?),
            _ => edge
                .unknown_children
                .push(cx.capture_any(pos, &e, is_empty)?),
        }
    }
}

fn parse_label(
    cx: &mut Cx<'_>,
    start: &BytesStart<'_>,
    empty: bool,
) -> Result<BpmnLabel, ParseError> {
    let mut set = attr_set(start)?;
    let mut label = BpmnLabel {
        id: set.take_id("id"),
        bounds: None,
    };

    if empty {
        return Ok(label);
    }

    loop {
        let (nsk, ev) = cx.read()?;
        let (e, is_empty) = match ev {
            Event::Start(e) => (e, false),
            Event::Empty(e) => (e, true),
            Event::End(_) => return Ok(label),
            Event::Eof => return Err(structure("unexpected end of file in BPMNLabel")),
            _ => continue,
        };

        if nsk == Ns::Dc && local(&e) == "Bounds" {
            label.bounds = Some(parse_bounds(&e)?);
        }
        // Per spec a label holds only bounds (plus extensions nobody emits);
        // anything else is consumed so the parse keeps its position.
        if !is_empty {
            cx.skip(&e)?;
        }
    }
}

/// Models a `conditionExpression` when it is text-only with at most an
/// `xsi:type` attribute; anything richer is preserved verbatim.
fn parse_condition(
    cx: &mut Cx<'_>,
    pos: usize,
    e: &BytesStart<'_>,
    empty: bool,
) -> Result<Condition, ParseError> {
    let mut set = attr_set(e)?;
    let xsi_type = set.take("xsi:type");
    if !set.is_empty() {
        return Ok(Condition::Raw(cx.capture_any(pos, e, empty)?));
    }

    if empty {
        let _ = cx.capture_span(pos);
        return Ok(Condition::Expression {
            xsi_type,
            text: String::new(),
        });
    }

    let raw = cx.capture(pos, e)?;
    let inner = raw
        .as_str()
        .find('>')
        .and_then(|open| raw.as_str().rfind("</").map(|close| (open, close)))
        .map(|(open, close)| &raw.as_str()[open + 1..close]);

    match inner {
        Some(inner) if !inner.contains('<') => Ok(Condition::Expression {
            xsi_type,
            text: quick_xml::escape::unescape(inner)
                .map_or_else(|_| inner.to_string(), std::borrow::Cow::into_owned),
        }),
        _ => Ok(Condition::Raw(raw)),
    }
}

/// Loop markers keep the original element verbatim unless it is a bare,
/// attribute-free tag the editor could have created itself.
fn parse_loop_marker(
    cx: &mut Cx<'_>,
    pos: usize,
    e: &BytesStart<'_>,
    empty: bool,
) -> Result<LoopMarker, ParseError> {
    let mut set = attr_set(e)?;
    let sequential = set.take_bool("isSequential").unwrap_or(false);
    let kind = if local(e) == "standardLoopCharacteristics" {
        LoopKind::Standard
    } else if sequential {
        LoopKind::MultiInstanceSequential
    } else {
        LoopKind::MultiInstanceParallel
    };

    let raw = cx.capture_any(pos, e, empty)?;
    let bare = empty && set.is_empty();
    Ok(LoopMarker {
        kind,
        raw: (!bare).then_some(raw),
    })
}
