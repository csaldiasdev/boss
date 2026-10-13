//! Round-trip tests against reference fixtures.

use bpmn_core::BpmnId;
use bpmn_core::model::{
    ActivityKind, EventDefinition, EventPosition, FlowElement, GatewayKind, RootElement, TaskType,
};
use bpmn_xml::{ParseError, parse, write};

const MINIMAL: &str = include_str!("../../../tests-fixtures/minimal.bpmn");
const ORDER: &str = include_str!("../../../tests-fixtures/order-fulfillment.bpmn");
const EXTENSIONS: &str = include_str!("../../../tests-fixtures/with-extensions.bpmn");
const PIZZA: &str = include_str!("../../../tests-fixtures/pizza-collaboration.bpmn");
const DATA: &str = include_str!("../../../tests-fixtures/data-artifacts.bpmn");
const PLANES: &str = include_str!("../../../tests-fixtures/collapsed-subprocess-plane.bpmn");

const FIXTURES: [(&str, &str); 6] = [
    ("minimal", MINIMAL),
    ("order-fulfillment", ORDER),
    ("with-extensions", EXTENSIONS),
    ("pizza-collaboration", PIZZA),
    ("data-artifacts", DATA),
    ("collapsed-subprocess-plane", PLANES),
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
            FlowElement::Data(_) | FlowElement::Artifact(_) => {}
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
fn parses_the_collaboration_fixture() {
    let doc = parse(PIZZA).expect("pizza fixture parses");

    let collaboration = doc.collaboration().expect("collaboration");
    assert_eq!(collaboration.participants.len(), 2);
    assert_eq!(collaboration.message_flows.len(), 2);
    assert_eq!(
        doc.participant(&BpmnId::from("Participant_Shop"))
            .and_then(|p| p.process_ref.clone()),
        Some(BpmnId::from("Process_Shop"))
    );

    let shop = doc
        .processes()
        .find(|p| p.base.id.as_str() == "Process_Shop")
        .expect("shop process");
    assert_eq!(shop.lane_sets.len(), 1);
    let lanes = &shop.lane_sets[0].lanes;
    assert_eq!(lanes.len(), 2);
    assert_eq!(lanes[0].name.as_deref(), Some("Kitchen"));
    assert_eq!(
        lanes[0].flow_node_refs,
        vec![
            BpmnId::from("Start_OrderReceived"),
            BpmnId::from("Task_Bake")
        ]
    );
    // The nested child lane set survives, as does the lane lookup through it.
    assert!(lanes[0].child_lane_set.is_some());
    assert_eq!(
        doc.lane(&BpmnId::from("Lane_Oven"))
            .and_then(|l| l.name.as_deref()),
        Some("Oven")
    );

    // Message flow endpoints resolve across pools.
    assert_eq!(
        doc.process_of(&BpmnId::from("Task_PlaceOrder")),
        Some(BpmnId::from("Process_Customer"))
    );

    // The writer keeps the vendor attribute and the lane structure.
    let out = write(&doc);
    for needle in [
        "acme:station=\"oven-2\"",
        "<bpmn:childLaneSet id=\"LaneSet_KitchenSplit\">",
        "<bpmn:flowNodeRef>Task_Deliver</bpmn:flowNodeRef>",
        "<bpmn:messageFlow id=\"MessageFlow_Order\" name=\"order\" sourceRef=\"Task_PlaceOrder\" targetRef=\"Start_OrderReceived\" />",
        "isHorizontal=\"true\"",
    ] {
        assert!(out.contains(needle), "output keeps {needle:?}\n---\n{out}");
    }
}

#[test]
fn parses_the_data_and_artifacts_fixture() {
    use bpmn_core::model::{ArtifactKind, DataKind};

    let doc = parse(DATA).expect("data fixture parses");
    let index = doc.flow_element_index();

    let FlowElement::Data(reference) = index[&BpmnId::from("DataObjectReference_Invoice")] else {
        panic!("data object reference expected");
    };
    assert_eq!(reference.name.as_deref(), Some("Invoice"));
    assert_eq!(
        reference.kind,
        DataKind::ObjectReference {
            data_object_ref: Some("DataObject_Invoice".into()),
        }
    );
    assert!(matches!(
        index[&BpmnId::from("DataStoreReference_Ledger")],
        FlowElement::Data(data) if matches!(
            &data.kind,
            DataKind::StoreReference { data_store_ref: Some(store) } if store.as_str() == "DataStore_Ledger"
        )
    ));

    let FlowElement::Artifact(annotation) = index[&BpmnId::from("TextAnnotation_1")] else {
        panic!("annotation expected");
    };
    assert!(matches!(
        &annotation.kind,
        ArtifactKind::TextAnnotation { text: Some(text) }
            if text == "Four-eyes principle applies & audit trail required"
    ));
    assert!(matches!(
        index[&BpmnId::from("Association_1")],
        FlowElement::Artifact(a) if matches!(
            &a.kind,
            ArtifactKind::Association { source, target, direction: Some(d) }
                if source.as_str() == "Task_Review"
                    && target.as_str() == "TextAnnotation_1"
                    && d == "None"
        )
    ));
    assert!(matches!(
        index[&BpmnId::from("Group_Billing")],
        FlowElement::Artifact(a) if matches!(
            &a.kind,
            ArtifactKind::Group { category_value_ref: Some(c) } if c == "CategoryValue_Billing"
        )
    ));

    // Execution-level data plumbing stays verbatim inside the task, and the
    // category/dataStore roots survive untouched.
    let out = write(&doc);
    for needle in [
        "<bpmn:ioSpecification id=\"IoSpec_1\">",
        "<bpmn:dataInputAssociation id=\"DataInputAssociation_1\">",
        "<bpmn:sourceRef>DataObjectReference_Invoice</bpmn:sourceRef>",
        "<bpmn:categoryValue id=\"CategoryValue_Billing\" value=\"Billing\" />",
        "<bpmn:dataStore id=\"DataStore_Ledger\" name=\"Ledger\" />",
        "<flow:assignment group=\"accounting\" />",
    ] {
        assert!(out.contains(needle), "output keeps {needle:?}\n---\n{out}");
    }
}

/// The M8 exit criterion: vendor and execution content survives a whole edit
/// session — open, edit unrelated things through the Document API, save.
#[test]
fn vendor_content_survives_an_edit_session() {
    use bpmn_core::NodeKind;
    use bpmn_core::geometry::Point;
    use std::collections::HashSet;

    let cases: [(&str, &str, &[&str]); 3] = [
        (
            "order-fulfillment",
            ORDER,
            &[
                "acme:assignee=\"demo\"",
                "<acme:formField id=\"approved\" label=\"Approve?\" type=\"boolean\" />",
                "xmlns:acme=\"https://example.com/schema/acme/bpmn\"",
            ],
        ),
        (
            "with-extensions",
            EXTENSIONS,
            &[
                "<flow:taskDefinition type=\"charge-card\" retries=\"3\" />",
                "meta:executionPlatform=\"Example Cloud\"",
                "PT5M",
            ],
        ),
        (
            "data-artifacts",
            DATA,
            &[
                "<bpmn:dataInputAssociation id=\"DataInputAssociation_1\">",
                "<flow:assignment group=\"accounting\" />",
                "<bpmn:categoryValue id=\"CategoryValue_Billing\" value=\"Billing\" />",
            ],
        ),
    ];

    for (name, input, needles) in cases {
        let mut doc = parse(input).unwrap_or_else(|e| panic!("{name} parses: {e}"));

        // A realistic session: add, rename, connect, move, undo-by-delete.
        let a = doc
            .create_node(
                NodeKind::Task(bpmn_core::model::TaskType::User),
                Point::new(900.0, 400.0),
            )
            .expect("create");
        let b = doc
            .create_node(NodeKind::EndEvent, Point::new(1100.0, 400.0))
            .expect("create");
        doc.set_name(&a, Some(String::from("Session task")));
        doc.connect(&a, &b).expect("connect");
        let moved: HashSet<_> = [a.clone()].into_iter().collect();
        doc.move_shapes(&moved, 15.0, -10.0);
        let doomed: HashSet<_> = [b].into_iter().collect();
        doc.delete_elements(&doomed);

        let out = write(&doc);
        for needle in needles {
            assert!(
                out.contains(needle),
                "{name}: vendor content lost after editing: {needle:?}"
            );
        }
        // And the edited document still parses as sound BPMN.
        parse(&out).unwrap_or_else(|e| panic!("{name} reparses after editing: {e}"));
    }
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
fn parses_the_secondary_plane_fixture() {
    let doc = parse(PLANES).expect("planes fixture parses");

    let diagrams = &doc.definitions.diagrams;
    assert_eq!(diagrams.len(), 2);
    assert_eq!(
        diagrams[1]
            .plane
            .as_ref()
            .and_then(|p| p.bpmn_element.as_ref()),
        Some(&BpmnId::from("Activity_Assess"))
    );
    // The subprocess children's DI lives only on the secondary plane.
    let on_root = |id: &str| {
        diagrams[0].plane.as_ref().is_some_and(|p| {
            p.elements
                .iter()
                .any(|e| e.bpmn_element() == Some(&BpmnId::from(id)))
        })
    };
    assert!(on_root("Activity_Assess"));
    assert!(!on_root("Task_CheckRecord"));

    // And it round-trips: both diagrams survive a write/parse cycle.
    let reparsed = parse(&write(&doc)).expect("written XML parses");
    assert_eq!(reparsed.definitions.diagrams.len(), 2);
}

#[test]
fn drill_down_scope_and_plane_migration() {
    use bpmn_core::model::SubProcessKind;
    use bpmn_core::{Document, NodeKind, Point, Scope};

    // A bpmn.io-style file already has the plane: drilling is a no-op.
    let mut doc = parse(PLANES).unwrap();
    let sub = BpmnId::from("Activity_Assess");
    let (index, changed) = doc.ensure_subprocess_plane(&sub).expect("is a subprocess");
    assert_eq!((index, changed), (1, false));

    // Creating a node in the drilled scope: semantics inside the subprocess,
    // DI on the subprocess's plane.
    let scope = Scope::Subprocess(sub.clone());
    let task = doc
        .create_node_in(
            &scope,
            NodeKind::Task(TaskType::Generic),
            Point::new(600.0, 180.0),
        )
        .expect("node created");
    assert_eq!(doc.container_of(&task), Some(sub.clone()));
    let on_plane = |doc: &Document, plane: usize, id: &BpmnId| {
        doc.definitions.diagrams[plane]
            .plane
            .as_ref()
            .is_some_and(|p| p.elements.iter().any(|e| e.bpmn_element() == Some(id)))
    };
    assert!(on_plane(&doc, 1, &task));
    assert!(!on_plane(&doc, 0, &task));

    // Collapse/expand migrate children DI between the parent plane and the
    // subprocess's own plane.
    let mut doc = Document::new();
    let sub = doc
        .create_node(
            NodeKind::SubProcess(SubProcessKind::Embedded),
            Point::new(300.0, 200.0),
        )
        .expect("subprocess created");
    let inner = doc
        .create_node(NodeKind::Task(TaskType::Generic), Point::new(300.0, 200.0))
        .expect("inner task lands in the expanded subprocess");
    assert_eq!(doc.container_of(&inner), Some(sub.clone()));

    assert!(doc.set_subprocess_expanded(&sub, false));
    assert_eq!(doc.definitions.diagrams.len(), 2);
    assert!(on_plane(&doc, 1, &inner));
    assert!(!on_plane(&doc, 0, &inner));
    // The migrated child no longer inflates the root view.
    let root_box = doc.content_bounds_in(&Scope::Root).unwrap();
    assert!(root_box.width <= 100.0 + f64::EPSILON);

    assert!(doc.set_subprocess_expanded(&sub, true));
    assert_eq!(doc.definitions.diagrams.len(), 1, "emptied diagram dropped");
    assert!(on_plane(&doc, 0, &inner));
    // The child sits inside the refitted shape.
    let (_, sub_bounds) = doc.node_outline(&sub).unwrap();
    let (_, inner_bounds) = doc.node_outline(&inner).unwrap();
    assert!(sub_bounds.contains(inner_bounds.center()));
}

#[test]
fn edits_reach_secondary_planes() {
    use std::collections::HashSet;

    let task = BpmnId::from("Task_CheckRecord");
    let sub = BpmnId::from("Activity_Assess");

    // Geometry readers see shapes on secondary planes.
    let doc = parse(PLANES).expect("planes fixture parses");
    let (_, bounds) = doc.node_outline(&task).expect("task shape found");
    assert_eq!((bounds.x, bounds.y), (280.0, 138.0));

    // Moving the collapsed subprocess carries its children's DI along,
    // wherever that DI lives.
    let mut doc = parse(PLANES).unwrap();
    let selection: HashSet<BpmnId> = [sub.clone()].into_iter().collect();
    doc.move_shapes(&selection, 50.0, 10.0);
    let (_, moved) = doc.node_outline(&task).unwrap();
    assert_eq!((moved.x, moved.y), (330.0, 148.0));

    // Extract copies nested DI from the secondary plane.
    let clipboard = doc.extract(&selection);
    assert!(clipboard.node_outline(&task).is_some());

    // Deleting the subprocess prunes its whole diagram and every shape.
    doc.delete_elements(&selection);
    assert_eq!(doc.definitions.diagrams.len(), 1);
    assert!(doc.node_outline(&task).is_none());
    assert!(doc.node_outline(&sub).is_none());
}

#[test]
fn multi_line_names_survive_the_round_trip() {
    let mut doc = parse(MINIMAL).expect("minimal fixture parses");
    let task = BpmnId::from("Activity_1");
    assert!(doc.set_name(&task, Some("Review\norder".to_string())));

    let xml = write(&doc);
    assert!(
        xml.contains("Review&#10;order"),
        "the attribute newline must be a character reference:\n{xml}"
    );
    assert!(
        !xml.contains("name=\"Review\norder"),
        "a literal newline would be normalized away on read:\n{xml}"
    );

    let reparsed = parse(&xml).expect("written XML parses");
    let index = reparsed.flow_element_index();
    assert_eq!(index[&task].name(), Some("Review\norder"));

    // Annotation text is element content; its newlines round-trip as-is.
    let mut doc = parse(DATA).expect("data fixture parses");
    let note = BpmnId::from("TextAnnotation_1");
    assert!(doc.set_name(&note, Some("first\nsecond".to_string())));
    let reparsed = parse(&write(&doc)).expect("written XML parses");
    assert_eq!(
        reparsed.flow_element_index()[&note].name(),
        Some("first\nsecond")
    );
}

#[test]
fn rejects_non_bpmn_xml() {
    assert!(matches!(
        parse("<foo><bar/></foo>"),
        Err(ParseError::MissingDefinitions)
    ));
    assert!(parse("not xml at all").is_err());
}
