//! Applying agent actions to a document.
//!
//! The one implementation both the app (inside its undoable edit session)
//! and the tests (against a bare document) run: the bridge carries actions,
//! this module gives them meaning. History actions (undo/redo) never reach
//! [`apply`] — the editor owns its history.

use crate::bridge::{ApplyOutcome, EditAction, NodeSpec, UpdatePatch};
use crate::placement;
use bpmn_core::{BpmnId, Document, NodeKind, Point, Scope};
use std::collections::HashSet;

/// Resolves where a new node goes: explicit center, after an anchor, inside
/// a container, or free space. Gives collapsed subprocess containers their
/// plane on the way.
fn resolve_spot(
    doc: &mut Document,
    kind: NodeKind,
    x: Option<f64>,
    y: Option<f64>,
    after: Option<&BpmnId>,
    container: Option<&BpmnId>,
) -> Result<(Scope, Point), String> {
    if let (Some(after), Some(container)) = (after, container)
        && doc.container_of(after).as_ref() != Some(container)
    {
        return Err(format!("{after} is not inside {container}"));
    }
    match (x, y) {
        // An explicit center keeps the classic rule: the point decides the
        // container, on the root plane.
        (Some(x), Some(y)) => Ok((Scope::Root, Point::new(x, y))),
        (None, None) => {
            if let Some(after) = after {
                placement::spot_after(doc, after, kind.default_size())
            } else {
                // A collapsed subprocess edits its own plane; make sure it
                // exists. Never for expanded ones — their children live on
                // the parent plane and the sweep would strip them off it.
                if let Some(container) = container
                    && !subprocess_is_expanded(doc, container)
                {
                    doc.ensure_subprocess_plane(container)
                        .ok_or_else(|| format!("{container} is not a subprocess"))?;
                }
                placement::spot_in(doc, container, kind.default_size())
            }
        }
        _ => Err(String::from("x and y go together: give both or neither")),
    }
}

/// Whether the subprocess's shape carries `isExpanded="true"`.
fn subprocess_is_expanded(doc: &Document, id: &BpmnId) -> bool {
    use bpmn_core::di::DiElement;
    doc.definitions.diagrams.iter().any(|diagram| {
        diagram.plane.as_ref().is_some_and(|plane| {
            plane.elements.iter().any(|element| {
                matches!(element, DiElement::Shape(shape)
                    if shape.bpmn_element == *id && shape.is_expanded == Some(true))
            })
        })
    })
}

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
#[expect(
    clippy::too_many_lines,
    reason = "one arm per action; a dispatch table"
)]
pub fn apply(action: EditAction, doc: &mut Document) -> Result<ApplyOutcome, String> {
    match action {
        EditAction::CreateNode {
            kind,
            x,
            y,
            after,
            container,
            name,
        } => {
            let (scope, center) =
                resolve_spot(doc, kind, x, y, after.as_ref(), container.as_ref())?;
            let created = doc.create_node_in(&scope, kind, center).ok_or_else(|| {
                String::from("could not create: in a collaboration, nodes go inside a pool")
            })?;
            if let Some(name) = name {
                doc.set_name(&created, Some(name));
            }
            match after {
                Some(after) => {
                    let flow = doc.connect(&after, &created).ok_or_else(|| {
                        format!("created {created}, but could not connect {after} to it")
                    })?;
                    Ok(ApplyOutcome::CreatedMany {
                        nodes: vec![created],
                        flows: vec![flow],
                    })
                }
                None => Ok(ApplyOutcome::Created(created)),
            }
        }
        EditAction::CreateSequence {
            nodes,
            after,
            container,
        } => {
            let Some(first) = nodes.first() else {
                return Ok(ApplyOutcome::Nothing);
            };
            let (scope, mut center) = resolve_spot(
                doc,
                first.kind,
                None,
                None,
                after.as_ref(),
                container.as_ref(),
            )?;

            let mut created: Vec<BpmnId> = Vec::with_capacity(nodes.len());
            let mut flows: Vec<BpmnId> = Vec::with_capacity(nodes.len());
            let mut previous_size = None;
            for NodeSpec { kind, name } in nodes {
                if let Some(previous) = previous_size {
                    center.x += placement::chain_step(previous, kind.default_size());
                }
                previous_size = Some(kind.default_size());
                let id = doc
                    .create_node_in(&scope, kind, center)
                    .ok_or_else(|| String::from("could not create a chain node"))?;
                if let Some(name) = name {
                    doc.set_name(&id, Some(name));
                }
                if let Some(previous) = created.last() {
                    let flow = doc
                        .connect(previous, &id)
                        .ok_or_else(|| format!("could not connect {previous} to {id}"))?;
                    flows.push(flow);
                }
                created.push(id);
            }
            if let (Some(after), Some(first)) = (after, created.first()) {
                let flow = doc
                    .connect(&after, first)
                    .ok_or_else(|| format!("could not connect {after} to {first}"))?;
                flows.insert(0, flow);
            }
            Ok(ApplyOutcome::CreatedMany {
                nodes: created,
                flows,
            })
        }
        EditAction::Update { id, patch } => {
            if !doc.flow_element_index().contains_key(&id)
                && doc.participant(&id).is_none()
                && doc.lane(&id).is_none()
                && doc.message_flows().all(|flow| flow.base.id != id)
            {
                return Err(format!("no element with id {id}"));
            }
            Ok(apply_patch(doc, &id, patch))
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

/// Applies the named fields of `patch`, collecting which changed and which
/// the document refused (wrong element kind, unknown flow, ...).
fn apply_patch(doc: &mut Document, id: &BpmnId, patch: UpdatePatch) -> ApplyOutcome {
    let mut applied: Vec<&'static str> = Vec::new();
    let mut failed: Vec<(&'static str, String)> = Vec::new();
    let mut record = |field: &'static str, ok: bool, reason: &str| {
        if ok {
            applied.push(field);
        } else {
            failed.push((field, reason.to_string()));
        }
    };

    if let Some(name) = patch.name {
        record("name", doc.set_name(id, name), "this element has no name");
    }
    if let Some(task_type) = patch.task_type {
        record("task_type", doc.set_task_type(id, task_type), "not a task");
    }
    if let Some(trigger) = patch.event_trigger {
        record(
            "event_trigger",
            doc.set_event_definition(id, trigger),
            "not an event",
        );
    }
    if let Some(default) = patch.gateway_default {
        record(
            "gateway_default",
            doc.set_gateway_default(id, default),
            "not a gateway, or the flow does not leave it",
        );
    }
    if let Some(condition) = patch.condition {
        record(
            "condition",
            doc.set_condition(id, condition),
            "not a sequence flow",
        );
    }
    if let Some(loop_marker) = patch.loop_marker {
        record(
            "loop",
            doc.set_loop_marker(id, loop_marker),
            "not an activity",
        );
    }
    if let Some(called) = patch.called_element {
        record(
            "called_element",
            doc.set_called_element(id, called),
            "not a call activity",
        );
    }
    if let Some(expanded) = patch.expanded {
        record(
            "expanded",
            doc.set_subprocess_expanded(id, expanded),
            "not a subprocess",
        );
    }

    ApplyOutcome::Updated { applied, failed }
}
