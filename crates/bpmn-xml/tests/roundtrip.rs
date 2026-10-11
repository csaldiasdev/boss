//! Round-trip tests against reference fixtures.

use bpmn_core::BpmnId;
use bpmn_core::model::{
    ActivityKind, EventDefinition, EventPosition, FlowElement, GatewayKind, RootElement, TaskType,
};
use bpmn_xml::{ParseError, parse, write};

const MINIMAL: &str = include_str!("../../../tests-fixtures/minimal.bpmn");
const ORDER: &str = include_str!("../../../tests-fixtures/order-fulfillment.bpmn");
const EXTENSIONS: &str = include_str!("../../../tests-fixtures/with-extensions.bpmn");

const FIXTURES: [(&str, &str); 3] = [
    ("minimal", MINIMAL),
    ("order-fulfillment", ORDER),
    ("with-extensions", EXTENSIONS),
];

#[test]
fn parses_the_order_fixture_structurally() {
    let doc = parse(ORDER).expect("order fixture parses");

    let process = doc.processes().next().expect("one process");
    assert_eq!(process.name.as_deref(), Some("Order fulfillment"));
    assert_eq!(process.is_executable, Some(true));
    assert_eq!(
        process.base.documentation,
        vec!["Ships an order when it is in stock.".to_string()]
    );

    let mut events = 0;
    let mut tasks = 0;
    let mut gateways = 0;
    let mut flows = 0;
    for element in &process.flow_elements {
        match element {
            FlowElement::Event(_) => events += 1,
            FlowElement::Activity(_) => tasks += 1,
            FlowElement::Gateway(_) => gateways += 1,
            FlowElement::SequenceFlow(_) => flows += 1,
            FlowElement::Unknown(raw) => panic!("unexpected unknown element: {}", raw.as_str()),
        }
    }
    assert_eq!((events, tasks, gateways, flows), (3, 3, 1, 6));

    let index = doc.flow_element_index();
    let review = index[&BpmnId::from("Task_Review")];
    let FlowElement::Activity(review) = review else {
        panic!("Task_Review is an activity");
    };
    assert_eq!(review.kind, ActivityKind::Task(TaskType::User));
    assert!(review.node.base.extension_elements.is_some());
    assert!(
        review
            .node
            .base
            .unknown_attrs
            .iter()
            .any(|(k, v)| k == "acme:assignee" && v == "demo")
    );

    let FlowElement::Gateway(gateway) = index[&BpmnId::from("Gateway_Stock")] else {
        panic!("Gateway_Stock is a gateway");
    };
    assert_eq!(gateway.kind, GatewayKind::Exclusive);
    assert_eq!(gateway.default_flow, Some("Flow_No".into()));

    let FlowElement::SequenceFlow(yes) = index[&BpmnId::from("Flow_Yes")] else {
        panic!("Flow_Yes is a sequence flow");
    };
    assert!(matches!(
        yes.condition.as_ref().unwrap(),
        bpmn_core::model::Condition::Expression { xsi_type: Some(t), text }
            if t == "bpmn:tFormalExpression" && text == "${inStock}"
    ));

    let plane = doc.diagram().unwrap().plane.as_ref().unwrap();
    assert_eq!(plane.elements.len(), 13);
}

#[test]
fn parses_extension_fixture_event_details() {
    let doc = parse(EXTENSIONS).expect("extensions fixture parses");
    let index = doc.flow_element_index();

    let FlowElement::Event(boundary) = index[&BpmnId::from("Boundary_Timeout")] else {
        panic!("boundary event expected");
    };
    assert_eq!(
        boundary.position,
        EventPosition::Boundary {
            attached_to: "Task_Charge".into(),
            cancel_activity: Some(false),
        }
    );
    assert!(matches!(
        boundary.definitions.as_slice(),
        [EventDefinition {
            kind: bpmn_core::model::EventDefinitionKind::Timer,
            body: bpmn_core::model::EventDefinitionBody::Raw(raw),
        }] if raw.as_str().contains("PT5M")
    ));

    let FlowElement::Event(catch) = index[&BpmnId::from("Catch_Order")] else {
        panic!("catch event expected");
    };
    assert!(matches!(
        catch.definitions.as_slice(),
        [EventDefinition {
            kind: bpmn_core::model::EventDefinitionKind::Message,
            body: bpmn_core::model::EventDefinitionBody::Message { message_ref: Some(m), .. },
        }] if m == "Message_Order"
    ));

    // The subprocess is modeled since M6: its contents nest under it.
    let FlowElement::Activity(sub) = index[&BpmnId::from("Sub_1")] else {
        panic!("Sub_1 is an activity");
    };
    let ActivityKind::SubProcess(sub) = &sub.kind else {
        panic!("Sub_1 is a subprocess");
    };
    assert_eq!(sub.kind, bpmn_core::model::SubProcessKind::Embedded);
    assert_eq!(sub.flow_elements.len(), 3, "start, end, and one flow");
    assert!(
        index.contains_key(&BpmnId::from("Sub_Start")),
        "nested ids indexed"
    );
    assert_eq!(
        doc.container_of(&BpmnId::from("Sub_Start")),
        Some(BpmnId::from("Sub_1"))
    );
    // Collapsed in DI, so its contents are hidden on the canvas.
    assert!(doc.hidden_by_collapse().contains(&BpmnId::from("Sub_Flow")));

    // The message root element survives too.
    assert!(doc.definitions.root_elements.iter().any(|root| {
        matches!(root, RootElement::Unknown(raw) if raw.as_str().contains("Message_Order"))
    }));
}

#[test]
fn round_trips_semantically() {
    for (name, input) in FIXTURES {
        let first = parse(input).unwrap_or_else(|e| panic!("{name} parses: {e}"));
        let written = write(&first);
        let second =
            parse(&written).unwrap_or_else(|e| panic!("{name} reparses: {e}\n---\n{written}"));
        assert_eq!(first, second, "{name}: model identical after round-trip");
    }
}

#[test]
fn write_is_idempotent() {
    for (name, input) in FIXTURES {
        let doc = parse(input).unwrap();
        let once = write(&doc);
        let twice = write(&parse(&once).unwrap());
        assert_eq!(once, twice, "{name}: second write is byte-identical");
    }
}

#[test]
fn vendor_extensions_survive_the_round_trip() {
    let doc = parse(ORDER).unwrap();
    let out = write(&doc);
    for needle in [
        "acme:assignee=\"demo\"",
        "acme:candidateGroups=\"sales\"",
        "<acme:formField id=\"approved\" label=\"Approve?\" type=\"boolean\" />",
        "<acme:property name=\"sla\" value=\"24h\" />",
        "xmlns:acme=\"https://example.com/schema/acme/bpmn\"",
        "${inStock}",
    ] {
        assert!(out.contains(needle), "output keeps {needle:?}\n---\n{out}");
    }

    let doc = parse(EXTENSIONS).unwrap();
    let out = write(&doc);
    for needle in [
        "<flow:taskDefinition type=\"charge-card\" retries=\"3\" />",
        "<flow:input source=\"=amount\" target=\"chargeAmount\" />",
        "meta:executionPlatform=\"Example Cloud\"",
        "<bpmn:subProcess id=\"Sub_1\"",
        "PT5M",
        "<!-- hand-made fixture exercising vendor extensions and unmodeled elements -->",
    ] {
        assert!(out.contains(needle), "output keeps {needle:?}\n---\n{out}");
    }
}

#[test]
fn rejects_non_bpmn_xml() {
    assert!(matches!(
        parse("<foo><bar/></foo>"),
        Err(ParseError::MissingDefinitions)
    ));
    assert!(parse("not xml at all").is_err());
}
