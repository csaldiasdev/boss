//! Behavior tests for the shared apply layer, run through the same bridge
//! the real server uses, against a headless editor loop.

mod support;

use boss_mcp::{ApplyOutcome, EditAction, NodeSpec, UpdatePatch};
use bpmn_core::model::{EventDefinitionKind, SubProcessKind, TaskType};
use bpmn_core::{BpmnId, Document, NodeKind, Scope};

fn task_at(x: f64, y: f64) -> EditAction {
    EditAction::CreateNode {
        kind: NodeKind::Task(TaskType::Generic),
        x: Some(x),
        y: Some(y),
        after: None,
        container: None,
        name: Some(String::from("A task")),
    }
}

fn created(outcome: Result<ApplyOutcome, String>) -> BpmnId {
    match outcome {
        Ok(ApplyOutcome::Created(id)) => id,
        other => panic!("expected a created id, got {other:?}"),
    }
}

#[tokio::test]
async fn create_connect_and_rename() {
    let editor = support::editor(Document::new());

    let a = created(editor.apply(task_at(200.0, 150.0)).await);
    let b = created(editor.apply(task_at(400.0, 150.0)).await);
    let Ok(ApplyOutcome::Created(flow)) = editor
        .apply(EditAction::Connect {
            source: a.clone(),
            target: b.clone(),
            name: Some(String::from("then")),
        })
        .await
    else {
        panic!("flow not created");
    };

    let doc = editor.snapshot().await.expect("snapshot");
    let index = doc.flow_element_index();
    assert_eq!(index[&a].name(), Some("A task"));
    assert_eq!(index[&flow].name(), Some("then"));
}

#[tokio::test]
async fn create_after_places_right_and_connects() {
    let editor = support::editor(Document::new());
    let a = created(editor.apply(task_at(200.0, 150.0)).await);

    let Ok(ApplyOutcome::CreatedMany { nodes, flows }) = editor
        .apply(EditAction::CreateNode {
            kind: NodeKind::EndEvent,
            x: None,
            y: None,
            after: Some(a.clone()),
            container: None,
            name: None,
        })
        .await
    else {
        panic!("after-creation did not report CreatedMany");
    };
    assert_eq!((nodes.len(), flows.len()), (1, 1));

    let doc = editor.snapshot().await.unwrap();
    let (_, a_bounds) = doc.node_outline(&a).unwrap();
    let (_, b_bounds) = doc.node_outline(&nodes[0]).unwrap();
    assert!(
        b_bounds.center().x >= a_bounds.center().x + 150.0 - f64::EPSILON,
        "placed right of the anchor"
    );
    assert!(
        doc.flow_element_index().contains_key(&flows[0]),
        "the connecting flow exists"
    );
}

#[tokio::test]
async fn create_sequence_builds_a_chain_in_one_undo_step() {
    let editor = support::editor(Document::new());

    let Ok(ApplyOutcome::CreatedMany { nodes, flows }) = editor
        .apply(EditAction::CreateSequence {
            nodes: vec![
                NodeSpec {
                    kind: NodeKind::StartEvent,
                    name: Some(String::from("Go")),
                },
                NodeSpec {
                    kind: NodeKind::Task(TaskType::User),
                    name: Some(String::from("Review")),
                },
                NodeSpec {
                    kind: NodeKind::EndEvent,
                    name: Some(String::from("Done")),
                },
            ],
            after: None,
            container: None,
        })
        .await
    else {
        panic!("sequence not created");
    };
    assert_eq!((nodes.len(), flows.len()), (3, 2));

    let doc = editor.snapshot().await.unwrap();
    assert_eq!(doc.flow_elements().count(), 5);

    // One undo reverts the whole chain.
    assert!(matches!(
        editor.apply(EditAction::Undo).await,
        Ok(ApplyOutcome::Changed)
    ));
    let doc = editor.snapshot().await.unwrap();
    assert_eq!(doc.flow_elements().count(), 0);
}

#[tokio::test]
async fn create_in_collapsed_subprocess_lands_on_its_plane() {
    let mut doc = Document::new();
    let sub = doc
        .create_node(
            NodeKind::SubProcess(SubProcessKind::Embedded),
            bpmn_core::Point::new(300.0, 200.0),
        )
        .unwrap();
    doc.set_subprocess_expanded(&sub, false);
    let editor = support::editor(doc);

    let Ok(ApplyOutcome::Created(inner)) = editor
        .apply(EditAction::CreateNode {
            kind: NodeKind::Task(TaskType::Generic),
            x: None,
            y: None,
            after: None,
            container: Some(sub.clone()),
            name: None,
        })
        .await
    else {
        panic!("inner task not created");
    };

    let doc = editor.snapshot().await.unwrap();
    assert_eq!(doc.container_of(&inner), Some(sub.clone()));
    let own = doc.plane_index(&Scope::Subprocess(sub)).expect("own plane");
    assert!(own > 0);
}

#[tokio::test]
async fn update_element_reports_applied_and_failed_fields() {
    let editor = support::editor(Document::new());
    let a = created(editor.apply(task_at(200.0, 150.0)).await);

    let Ok(ApplyOutcome::Updated { applied, failed }) = editor
        .apply(EditAction::Update {
            id: a.clone(),
            patch: UpdatePatch {
                name: Some(Some(String::from("Check stock"))),
                task_type: Some(TaskType::Service),
                // A task has no trigger: this field must fail, alone.
                event_trigger: Some(Some(EventDefinitionKind::Timer)),
                ..UpdatePatch::default()
            },
        })
        .await
    else {
        panic!("update did not report");
    };
    assert_eq!(applied, vec!["name", "task_type"]);
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0].0, "event_trigger");

    let doc = editor.snapshot().await.unwrap();
    assert_eq!(doc.flow_element_index()[&a].name(), Some("Check stock"));

    let error = editor
        .apply(EditAction::Update {
            id: BpmnId::from("Ghost_1"),
            patch: UpdatePatch::default(),
        })
        .await
        .expect_err("patching an unknown id must refuse");
    assert!(error.contains("no element"));
}

#[tokio::test]
async fn bulk_actions_report_unknown_ids_and_noops_leave_no_undo_step() {
    let editor = support::editor(Document::new());

    let a = created(editor.apply(task_at(200.0, 150.0)).await);

    // A delete naming one real and one bogus id reports both sides.
    let Ok(ApplyOutcome::Affected { affected, unknown }) = editor
        .apply(EditAction::Delete {
            ids: vec![a.clone(), BpmnId::from("Ghost_1")],
        })
        .await
    else {
        panic!("delete did not report");
    };
    assert_eq!(affected, vec![a]);
    assert_eq!(unknown, vec![BpmnId::from("Ghost_1")]);

    // An all-unknown move changes nothing and records no undo step: the next
    // undo reverts the delete above, bringing the task back.
    let Ok(outcome) = editor
        .apply(EditAction::Move {
            ids: vec![BpmnId::from("Ghost_2")],
            dx: 10.0,
            dy: 10.0,
        })
        .await
    else {
        panic!("move refused");
    };
    assert!(!outcome.changed_document());

    assert!(matches!(
        editor.apply(EditAction::Undo).await,
        Ok(ApplyOutcome::Changed)
    ));
    let doc = editor.snapshot().await.expect("snapshot");
    assert_eq!(doc.flow_elements().count(), 1, "the delete was undone");
}

#[tokio::test]
async fn refusals_carry_reasons() {
    let editor = support::editor(Document::new());

    let error = editor
        .apply(EditAction::Connect {
            source: BpmnId::from("Nope_1"),
            target: BpmnId::from("Nope_2"),
            name: None,
        })
        .await
        .expect_err("connecting unknown ids must refuse");
    assert!(error.contains("cannot connect"));
}
