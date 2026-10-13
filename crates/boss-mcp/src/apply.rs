//! Applying agent actions to a document.
//!
//! The one implementation both the app (inside its undoable edit session)
//! and the tests (against a bare document) run: the bridge carries actions,
//! this module gives them meaning. History actions (undo/redo) never reach
//! [`apply`] — the editor owns its history.

use crate::bridge::{ApplyOutcome, EditAction};
use bpmn_core::{BpmnId, Document, Point};
use std::collections::HashSet;

/// Splits `ids` into those the document knows and those it does not.
fn partition_known(doc: &Document, ids: Vec<BpmnId>) -> (Vec<BpmnId>, Vec<BpmnId>) {
    let index = doc.flow_element_index();
    ids.into_iter().partition(|id| {
        index.contains_key(id)
            || doc.participant(id).is_some()
            || doc.lane(id).is_some()
            || doc.message_flows().any(|flow| flow.base.id == *id)
    })
}

/// Applies one agent action to the document. `Err` is a human-readable
/// refusal, after which the document must be treated as unchanged.
pub fn apply(action: EditAction, doc: &mut Document) -> Result<ApplyOutcome, String> {
    match action {
        EditAction::CreateNode { kind, x, y, name } => {
            let created = doc.create_node(kind, Point::new(x, y)).ok_or_else(|| {
                String::from("could not create: in a collaboration, nodes go inside a pool")
            })?;
            if let Some(name) = name {
                doc.set_name(&created, Some(name));
            }
            Ok(ApplyOutcome::Created(created))
        }
        EditAction::CreateBoundary { host, x, y } => doc
            .create_boundary_event(&host, Point::new(x, y))
            .map(ApplyOutcome::Created)
            .ok_or_else(|| {
                format!("{host} is not an activity; boundary events attach to activities")
            }),
        EditAction::CreatePool { name, x, y } => doc
            .create_pool(name, Point::new(x, y))
            .map(ApplyOutcome::Created)
            .ok_or_else(|| String::from("could not create a pool")),
        EditAction::AddLane { pool, name } => doc
            .add_lane(&pool, name)
            .map(ApplyOutcome::Created)
            .ok_or_else(|| format!("{pool} is not a pool backed by a process")),
        EditAction::Connect {
            source,
            target,
            name,
        } => {
            let created = doc.connect(&source, &target).ok_or_else(|| {
                String::from("cannot connect: unknown endpoint ids or a self-connection")
            })?;
            if let Some(name) = name {
                doc.set_name(&created, Some(name));
            }
            Ok(ApplyOutcome::Created(created))
        }
        EditAction::SetName { id, name } => {
            if doc.set_name(&id, name) {
                Ok(ApplyOutcome::Changed)
            } else {
                Err(format!("no element with id {id}"))
            }
        }
        EditAction::Delete { ids } => {
            let (known, unknown) = partition_known(doc, ids);
            if !known.is_empty() {
                let doomed: HashSet<BpmnId> = known.iter().cloned().collect();
                doc.delete_elements(&doomed);
            }
            Ok(ApplyOutcome::Affected {
                affected: known,
                unknown,
            })
        }
        EditAction::Move { ids, dx, dy } => {
            let (known, unknown) = partition_known(doc, ids);
            if !known.is_empty() {
                let moving: HashSet<BpmnId> = known.iter().cloned().collect();
                doc.move_shapes(&moving, dx, dy);
            }
            Ok(ApplyOutcome::Affected {
                affected: known,
                unknown,
            })
        }
        EditAction::Replace { document } => {
            *doc = *document;
            Ok(ApplyOutcome::Changed)
        }
        EditAction::Undo | EditAction::Redo => {
            Err(String::from("history actions belong to the editor"))
        }
    }
}
