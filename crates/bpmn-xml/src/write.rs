//! The BPMN writer: a small hand-rolled emitter.
//!
//! Hand-rolled because raw fragments must be injected verbatim mid-document,
//! which fights every streaming writer API. Output is Camunda-style: 2-space
//! indent, one element per line, standard namespaces under the prefixes the
//! source file used (conventional prefixes for new documents).

use crate::ns;
use bpmn_core::di::{BpmnDiagram, BpmnEdge, BpmnLabel, BpmnPlane, BpmnShape, DiElement};
use bpmn_core::geometry::Bounds;
use bpmn_core::model::{
    Activity, ActivityKind, BaseElement, Condition, Definitions, Event, EventDefinition,
    EventDefinitionBody, EventPosition, FlowElement, FlowNode, Gateway, GatewayKind, LoopKind,
    LoopMarker, Process, RootElement, SequenceFlow, TaskType,
};
use bpmn_core::{BpmnId, Document, RawXml};
use quick_xml::escape::escape;

/// Serializes a document to BPMN 2.0 XML.
pub fn write(doc: &Document) -> String {
    let prefixes = Prefixes::resolve(&doc.definitions);
    let mut em = Emitter {
        out: String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>"),
        depth: 0,
        prefixes,
    };
    em.definitions(&doc.definitions);
    em.out.push('\n');
    em.out
}

/// How one of the standard namespaces is written.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Binding {
    /// Bound as the default namespace: bare local names.
    Default,
    /// Bound under a prefix.
    Prefix(String),
}

impl Binding {
    fn qualify(&self, local: &str) -> String {
        match self {
            Binding::Default => local.to_string(),
            Binding::Prefix(prefix) => format!("{prefix}:{local}"),
        }
    }
}

struct Prefixes {
    model: Binding,
    bpmndi: Binding,
    dc: Binding,
    di: Binding,
    /// Declarations to append to the root for standard namespaces the source
    /// document did not declare (new/synthesized documents).
    extra_decls: Vec<(String, String)>,
}

impl Prefixes {
    fn resolve(def: &Definitions) -> Self {
        let mut extra_decls = Vec::new();

        let mut lookup = |uri: &str, conventional: &str| {
            for (prefix, declared) in &def.ns_decls {
                if declared == uri {
                    return match prefix {
                        None => Binding::Default,
                        Some(p) => Binding::Prefix(p.clone()),
                    };
                }
            }
            // Not declared: pick the conventional prefix, dodging collisions.
            let mut candidate = conventional.to_string();
            while def
                .ns_decls
                .iter()
                .any(|(p, _)| p.as_deref() == Some(candidate.as_str()))
            {
                candidate.push('_');
            }
            extra_decls.push((candidate.clone(), uri.to_string()));
            Binding::Prefix(candidate)
        };

        let [model, bpmndi, dc, di] =
            ns::STANDARD.map(|(conventional, uri)| lookup(uri, conventional));

        Self {
            model,
            bpmndi,
            dc,
            di,
            extra_decls,
        }
    }
}

/// Attribute list under construction; values are escaped on push.
#[derive(Default)]
struct Attrs(Vec<(String, String)>);

impl Attrs {
    fn push(&mut self, key: &str, value: &str) {
        self.0.push((key.to_string(), escape(value).into_owned()));
    }

    fn push_opt(&mut self, key: &str, value: Option<&str>) {
        if let Some(value) = value {
            self.push(key, value);
        }
    }

    fn push_id(&mut self, key: &str, id: &BpmnId) {
        if !id.as_str().is_empty() {
            self.push(key, id.as_str());
        }
    }

    fn push_id_opt(&mut self, key: &str, id: Option<&BpmnId>) {
        if let Some(id) = id {
            self.push_id(key, id);
        }
    }

    fn push_bool_opt(&mut self, key: &str, value: Option<bool>) {
        if let Some(value) = value {
            self.push(key, if value { "true" } else { "false" });
        }
    }

    fn push_unknown(&mut self, unknown: &[(String, String)]) {
        for (key, value) in unknown {
            self.push(key, value);
        }
    }
}

fn fmt_num(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 9e15 {
        // Camunda writes whole coordinates without a decimal point.
        format!("{}", value as i64)
    } else {
        format!("{value}")
    }
}

struct Emitter {
    out: String,
    depth: usize,
    prefixes: Prefixes,
}

impl Emitter {
    fn newline(&mut self) {
        self.out.push('\n');
        for _ in 0..self.depth {
            self.out.push_str("  ");
        }
    }

    fn tag(&mut self, name: &str, attrs: &Attrs, self_close: bool) {
        self.newline();
        self.out.push('<');
        self.out.push_str(name);
        for (key, value) in &attrs.0 {
            self.out.push(' ');
            self.out.push_str(key);
            self.out.push_str("=\"");
            self.out.push_str(value);
            self.out.push('"');
        }
        if self_close {
            self.out.push_str(" />");
        } else {
            self.out.push('>');
            self.depth += 1;
        }
    }

    fn open(&mut self, name: &str, attrs: &Attrs) {
        self.tag(name, attrs, false);
    }

    fn empty(&mut self, name: &str, attrs: &Attrs) {
        self.tag(name, attrs, true);
    }

    fn close(&mut self, name: &str) {
        self.depth -= 1;
        self.newline();
        self.out.push_str("</");
        self.out.push_str(name);
        self.out.push('>');
    }

    /// An element with text content on a single line, self-closing when empty.
    fn text_element(&mut self, name: &str, text: &str) {
        if text.is_empty() {
            self.empty(name, &Attrs::default());
        } else {
            self.newline();
            self.out.push('<');
            self.out.push_str(name);
            self.out.push('>');
            self.out.push_str(&escape(text));
            self.out.push_str("</");
            self.out.push_str(name);
            self.out.push('>');
        }
    }

    /// A text element that may carry attributes, self-closing when empty.
    fn attr_text_element(&mut self, name: &str, attrs: &Attrs, text: &str) {
        if text.is_empty() {
            self.empty(name, attrs);
            return;
        }
        self.tag(name, attrs, false);
        self.depth -= 1;
        self.out.push_str(&escape(text));
        self.out.push_str("</");
        self.out.push_str(name);
        self.out.push('>');
    }

    /// Verbatim fragment on its own line; internal formatting is preserved.
    fn raw(&mut self, raw: &RawXml) {
        self.newline();
        self.out.push_str(raw.as_str());
    }

    fn model_tag(&self, local: &str) -> String {
        self.prefixes.model.qualify(local)
    }

    // -- definitions ---------------------------------------------------------

    fn definitions(&mut self, def: &Definitions) {
        let mut attrs = Attrs::default();
        for (prefix, uri) in &def.ns_decls {
            match prefix {
                None => attrs.push("xmlns", uri),
                Some(p) => attrs.push(&format!("xmlns:{p}"), uri),
            }
        }
        for (prefix, uri) in &self.prefixes.extra_decls.clone() {
            attrs.push(&format!("xmlns:{prefix}"), uri);
        }
        attrs.push_id_opt("id", def.id.as_ref());
        if !def.target_namespace.is_empty() {
            attrs.push("targetNamespace", &def.target_namespace);
        }
        attrs.push_opt("exporter", def.exporter.as_deref());
        attrs.push_opt("exporterVersion", def.exporter_version.as_deref());
        attrs.push_unknown(&def.unknown_attrs);

        let name = self.model_tag("definitions");
        self.open(&name, &attrs);
        for root in &def.root_elements {
            match root {
                RootElement::Process(process) => self.process(process),
                RootElement::Unknown(raw) => self.raw(raw),
            }
        }
        for diagram in &def.diagrams {
            self.diagram(diagram);
        }
        self.close(&name);
    }

    // -- semantic elements ---------------------------------------------------

    fn base_children(&mut self, base: &BaseElement) {
        let doc_tag = self.model_tag("documentation");
        for documentation in &base.documentation {
            self.text_element(&doc_tag, documentation);
        }
        if let Some(extension) = &base.extension_elements {
            self.raw(extension);
        }
    }

    fn node_children(&mut self, node: &FlowNode) {
        self.base_children(&node.base);
        let incoming_tag = self.model_tag("incoming");
        for id in &node.incoming {
            self.text_element(&incoming_tag, id.as_str());
        }
        let outgoing_tag = self.model_tag("outgoing");
        for id in &node.outgoing {
            self.text_element(&outgoing_tag, id.as_str());
        }
    }

    fn node_has_children(node: &FlowNode) -> bool {
        !node.base.documentation.is_empty()
            || node.base.extension_elements.is_some()
            || !node.base.unknown_children.is_empty()
            || !node.incoming.is_empty()
            || !node.outgoing.is_empty()
    }

    fn node_attrs(node: &FlowNode) -> Attrs {
        let mut attrs = Attrs::default();
        attrs.push_id("id", &node.base.id);
        attrs.push_opt("name", node.name.as_deref());
        attrs
    }

    fn process(&mut self, process: &Process) {
        let mut attrs = Attrs::default();
        attrs.push_id("id", &process.base.id);
        attrs.push_opt("name", process.name.as_deref());
        attrs.push_bool_opt("isExecutable", process.is_executable);
        attrs.push_unknown(&process.base.unknown_attrs);

        let has_children = !process.base.documentation.is_empty()
            || process.base.extension_elements.is_some()
            || !process.base.unknown_children.is_empty()
            || !process.flow_elements.is_empty();

        let name = self.model_tag("process");
        if !has_children {
            self.empty(&name, &attrs);
            return;
        }

        self.open(&name, &attrs);
        self.base_children(&process.base);
        for element in &process.flow_elements {
            match element {
                FlowElement::Event(event) => self.event(event),
                FlowElement::Activity(activity) => self.activity(activity),
                FlowElement::Gateway(gateway) => self.gateway(gateway),
                FlowElement::SequenceFlow(flow) => self.sequence_flow(flow),
                FlowElement::Unknown(raw) => self.raw(raw),
            }
        }
        for raw in &process.base.unknown_children {
            self.raw(raw);
        }
        self.close(&name);
    }

    fn event(&mut self, event: &Event) {
        let local = match &event.position {
            EventPosition::Start { .. } => "startEvent",
            EventPosition::End => "endEvent",
            EventPosition::IntermediateCatch => "intermediateCatchEvent",
            EventPosition::IntermediateThrow => "intermediateThrowEvent",
            EventPosition::Boundary { .. } => "boundaryEvent",
        };
        let mut attrs = Self::node_attrs(&event.node);
        match &event.position {
            EventPosition::Start { is_interrupting } => {
                attrs.push_bool_opt("isInterrupting", *is_interrupting);
            }
            EventPosition::Boundary {
                attached_to,
                cancel_activity,
            } => {
                attrs.push_id("attachedToRef", attached_to);
                attrs.push_bool_opt("cancelActivity", *cancel_activity);
            }
            _ => {}
        }
        attrs.push_unknown(&event.node.base.unknown_attrs);

        let name = self.model_tag(local);
        if !Self::node_has_children(&event.node) && event.definitions.is_empty() {
            self.empty(&name, &attrs);
            return;
        }

        self.open(&name, &attrs);
        self.node_children(&event.node);
        for definition in &event.definitions {
            self.event_definition(definition);
        }
        for raw in &event.node.base.unknown_children {
            self.raw(raw);
        }
        self.close(&name);
    }

    fn event_definition(&mut self, definition: &EventDefinition) {
        match (&definition.body, definition.kind.tag()) {
            (EventDefinitionBody::Message { id, message_ref }, Some(tag)) => {
                let mut attrs = Attrs::default();
                attrs.push_id_opt("id", id.as_ref());
                attrs.push_opt("messageRef", message_ref.as_deref());
                let tag = self.model_tag(tag);
                self.empty(&tag, &attrs);
            }
            (EventDefinitionBody::Empty { id }, Some(tag)) => {
                let mut attrs = Attrs::default();
                attrs.push_id_opt("id", id.as_ref());
                let tag = self.model_tag(tag);
                self.empty(&tag, &attrs);
            }
            (EventDefinitionBody::Raw(raw), _) => self.raw(raw),
            // A structured body with no tag cannot happen (kind Other always
            // carries Raw); emit nothing rather than invent markup.
            (_, None) => {}
        }
    }

    fn loop_marker(&mut self, marker: &LoopMarker) {
        if let Some(raw) = &marker.raw {
            self.raw(raw);
            return;
        }
        let (tag, sequential) = match marker.kind {
            LoopKind::Standard => ("standardLoopCharacteristics", false),
            LoopKind::MultiInstanceParallel => ("multiInstanceLoopCharacteristics", false),
            LoopKind::MultiInstanceSequential => ("multiInstanceLoopCharacteristics", true),
        };
        let mut attrs = Attrs::default();
        if sequential {
            attrs.push("isSequential", "true");
        }
        let tag = self.model_tag(tag);
        self.empty(&tag, &attrs);
    }

    fn activity(&mut self, activity: &Activity) {
        let ActivityKind::Task(task_type) = activity.kind;
        let local = match task_type {
            TaskType::Generic => "task",
            TaskType::User => "userTask",
            TaskType::Service => "serviceTask",
            TaskType::Script => "scriptTask",
            TaskType::Manual => "manualTask",
            TaskType::Send => "sendTask",
            TaskType::Receive => "receiveTask",
            TaskType::BusinessRule => "businessRuleTask",
        };
        let mut attrs = Self::node_attrs(&activity.node);
        attrs.push_unknown(&activity.node.base.unknown_attrs);

        let name = self.model_tag(local);
        if !Self::node_has_children(&activity.node) && activity.loop_marker.is_none() {
            self.empty(&name, &attrs);
            return;
        }

        self.open(&name, &attrs);
        self.node_children(&activity.node);
        if let Some(marker) = &activity.loop_marker {
            self.loop_marker(marker);
        }
        for raw in &activity.node.base.unknown_children {
            self.raw(raw);
        }
        self.close(&name);
    }

    fn gateway(&mut self, gateway: &Gateway) {
        let local = match gateway.kind {
            GatewayKind::Exclusive => "exclusiveGateway",
            GatewayKind::Parallel => "parallelGateway",
            GatewayKind::Inclusive => "inclusiveGateway",
            GatewayKind::EventBased => "eventBasedGateway",
            GatewayKind::Complex => "complexGateway",
        };
        let mut attrs = Self::node_attrs(&gateway.node);
        attrs.push_id_opt("default", gateway.default_flow.as_ref());
        attrs.push_bool_opt("instantiate", gateway.instantiate);
        attrs.push_unknown(&gateway.node.base.unknown_attrs);

        let name = self.model_tag(local);
        if !Self::node_has_children(&gateway.node) {
            self.empty(&name, &attrs);
            return;
        }

        self.open(&name, &attrs);
        self.node_children(&gateway.node);
        for raw in &gateway.node.base.unknown_children {
            self.raw(raw);
        }
        self.close(&name);
    }

    fn sequence_flow(&mut self, flow: &SequenceFlow) {
        let mut attrs = Attrs::default();
        attrs.push_id("id", &flow.base.id);
        attrs.push_opt("name", flow.name.as_deref());
        attrs.push_id("sourceRef", &flow.source);
        attrs.push_id("targetRef", &flow.target);
        attrs.push_unknown(&flow.base.unknown_attrs);

        let has_children = !flow.base.documentation.is_empty()
            || flow.base.extension_elements.is_some()
            || flow.condition.is_some()
            || !flow.base.unknown_children.is_empty();

        let name = self.model_tag("sequenceFlow");
        if !has_children {
            self.empty(&name, &attrs);
            return;
        }

        self.open(&name, &attrs);
        self.base_children(&flow.base);
        match &flow.condition {
            Some(Condition::Expression { xsi_type, text }) => {
                let tag = self.model_tag("conditionExpression");
                let mut attrs = Attrs::default();
                if let Some(xsi_type) = xsi_type {
                    attrs.push("xsi:type", xsi_type);
                }
                self.attr_text_element(&tag, &attrs, text);
            }
            Some(Condition::Raw(raw)) => self.raw(raw),
            None => {}
        }
        for raw in &flow.base.unknown_children {
            self.raw(raw);
        }
        self.close(&name);
    }

    // -- BPMN DI -------------------------------------------------------------

    fn bounds(&mut self, bounds: &Bounds) {
        let mut attrs = Attrs::default();
        attrs.push("x", &fmt_num(bounds.x));
        attrs.push("y", &fmt_num(bounds.y));
        attrs.push("width", &fmt_num(bounds.width));
        attrs.push("height", &fmt_num(bounds.height));
        let tag = self.prefixes.dc.qualify("Bounds");
        self.empty(&tag, &attrs);
    }

    fn label(&mut self, label: &BpmnLabel) {
        let mut attrs = Attrs::default();
        attrs.push_id_opt("id", label.id.as_ref());
        let tag = self.prefixes.bpmndi.qualify("BPMNLabel");
        match &label.bounds {
            Some(bounds) => {
                self.open(&tag, &attrs);
                self.bounds(bounds);
                self.close(&tag);
            }
            None => self.empty(&tag, &attrs),
        }
    }

    fn diagram(&mut self, diagram: &BpmnDiagram) {
        let mut attrs = Attrs::default();
        attrs.push_id_opt("id", diagram.id.as_ref());
        attrs.push_opt("name", diagram.name.as_deref());
        attrs.push_unknown(&diagram.unknown_attrs);

        let tag = self.prefixes.bpmndi.qualify("BPMNDiagram");
        if diagram.plane.is_none() && diagram.unknown_children.is_empty() {
            self.empty(&tag, &attrs);
            return;
        }

        self.open(&tag, &attrs);
        if let Some(plane) = &diagram.plane {
            self.plane(plane);
        }
        for raw in &diagram.unknown_children {
            self.raw(raw);
        }
        self.close(&tag);
    }

    fn plane(&mut self, plane: &BpmnPlane) {
        let mut attrs = Attrs::default();
        attrs.push_id_opt("id", plane.id.as_ref());
        attrs.push_id_opt("bpmnElement", plane.bpmn_element.as_ref());
        attrs.push_unknown(&plane.unknown_attrs);

        let tag = self.prefixes.bpmndi.qualify("BPMNPlane");
        if plane.elements.is_empty() {
            self.empty(&tag, &attrs);
            return;
        }

        self.open(&tag, &attrs);
        for element in &plane.elements {
            match element {
                DiElement::Shape(shape) => self.shape(shape),
                DiElement::Edge(edge) => self.edge(edge),
                DiElement::Unknown(raw) => self.raw(raw),
            }
        }
        self.close(&tag);
    }

    fn shape(&mut self, shape: &BpmnShape) {
        let mut attrs = Attrs::default();
        attrs.push_id_opt("id", shape.id.as_ref());
        attrs.push_id("bpmnElement", &shape.bpmn_element);
        attrs.push_bool_opt("isMarkerVisible", shape.is_marker_visible);
        attrs.push_bool_opt("isExpanded", shape.is_expanded);
        attrs.push_bool_opt("isHorizontal", shape.is_horizontal);
        attrs.push_unknown(&shape.unknown_attrs);

        let tag = self.prefixes.bpmndi.qualify("BPMNShape");
        self.open(&tag, &attrs);
        self.bounds(&shape.bounds);
        if let Some(label) = &shape.label {
            self.label(label);
        }
        for raw in &shape.unknown_children {
            self.raw(raw);
        }
        self.close(&tag);
    }

    fn edge(&mut self, edge: &BpmnEdge) {
        let mut attrs = Attrs::default();
        attrs.push_id_opt("id", edge.id.as_ref());
        attrs.push_id("bpmnElement", &edge.bpmn_element);
        attrs.push_unknown(&edge.unknown_attrs);

        let tag = self.prefixes.bpmndi.qualify("BPMNEdge");
        self.open(&tag, &attrs);
        let waypoint_tag = self.prefixes.di.qualify("waypoint");
        for point in &edge.waypoints {
            let mut attrs = Attrs::default();
            attrs.push("x", &fmt_num(point.x));
            attrs.push("y", &fmt_num(point.y));
            self.empty(&waypoint_tag, &attrs);
        }
        if let Some(label) = &edge.label {
            self.label(label);
        }
        for raw in &edge.unknown_children {
            self.raw(raw);
        }
        self.close(&tag);
    }
}
