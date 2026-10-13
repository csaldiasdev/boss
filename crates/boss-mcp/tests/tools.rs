//! Behavior tests for the shared apply layer, run through the same bridge
//! the real server uses, against a headless editor loop.

mod support;

use boss_mcp::{ApplyOutcome, EditAction};
use bpmn_core::model::TaskType;
use bpmn_core::{BpmnId, Document, NodeKind};

fn task(x: f64, y: f64) -> EditAction {
    EditAction::CreateNode {
        kind: NodeKind::Task(TaskType::Generic),
        x,
        y,
        name: Some(String::from("A task")),
    }
}

#[tokio::test]
async fn create_connect_and_rename() {
    let editor = support::editor(Document::new());

    let Ok(ApplyOutcome::Created(a)) = editor.apply(task(200.0, 150.0)).await else {
        panic!("first task not created");
    };
    let Ok(ApplyOutcome::Created(b)) = editor.apply(task(400.0, 150.0)).await else {
        panic!("second task not created");
    };
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
async fn bulk_actions_report_unknown_ids_and_noops_leave_no_undo_step() {
    let editor = support::editor(Document::new());

    let Ok(ApplyOutcome::Created(a)) = editor.apply(task(200.0, 150.0)).await else {
        panic!("task not created");
    };

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

    let error = editor
        .apply(EditAction::SetName {
            id: BpmnId::from("Nope_1"),
            name: Some(String::from("x")),
        })
        .await
        .expect_err("renaming an unknown id must refuse");
    assert!(error.contains("no element"));
}
