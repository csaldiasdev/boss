//! Automatic node placement, so agents rarely need coordinates.
//!
//! Pure geometry over a document snapshot: callers pick the anchor (`after` a
//! node, inside a `container`, or free space right of the content) and get a
//! scope plus a center back. Containers that merely surround nodes (pools,
//! lanes, expanded subprocesses, groups) do not count as obstacles.

use bpmn_core::di::DiElement;
use bpmn_core::model::{ActivityKind, ArtifactKind, FlowElement};
use bpmn_core::{Bounds, BpmnId, Document, Point, Scope, Size};

/// Horizontal center-to-center step of a left-to-right chain.
const STEP: f64 = 150.0;
/// Clearance kept between distinct shapes.
const GAP: f64 = 50.0;
/// Where content starts on an empty plane.
const ORIGIN: Point = Point { x: 200.0, y: 150.0 };

/// The scope whose plane draws `id`'s shape.
fn scope_of_shape(doc: &Document, id: &BpmnId) -> Option<Scope> {
    for (index, diagram) in doc.definitions.diagrams.iter().enumerate() {
        let Some(plane) = diagram.plane.as_ref() else {
            continue;
        };
        let holds = plane
            .elements
            .iter()
            .any(|element| matches!(element, DiElement::Shape(shape) if shape.bpmn_element == *id));
        if holds {
            if index == 0 {
                return Some(Scope::Root);
            }
            return Some(
                plane
                    .bpmn_element
                    .clone()
                    .map_or(Scope::Root, Scope::Subprocess),
            );
        }
    }
    None
}

/// The shapes a new node must not overlap on `scope`'s plane: nodes, not the
/// containers around them.
fn obstacles(doc: &Document, scope: &Scope) -> Vec<Bounds> {
    let index = doc.flow_element_index();
    let Some(plane) = doc.plane_in(scope) else {
        return Vec::new();
    };
    plane
        .elements
        .iter()
        .filter_map(|element| match element {
            DiElement::Shape(shape) => {
                let contains_others = match index.get(&shape.bpmn_element) {
                    Some(FlowElement::Activity(activity)) => {
                        matches!(activity.kind, ActivityKind::SubProcess(_))
                            && shape.is_expanded == Some(true)
                    }
                    Some(FlowElement::Artifact(artifact)) => {
                        matches!(artifact.kind, ArtifactKind::Group { .. })
                    }
                    // Pools and lanes are not flow elements.
                    None => true,
                    _ => false,
                };
                (!contains_others).then_some(shape.bounds)
            }
            _ => None,
        })
        .collect()
}

/// Nudges `center` down until a `size` shape there clears every obstacle.
fn clear_of(obstacles: &[Bounds], mut center: Point, size: Size) -> Point {
    for _ in 0..50 {
        let candidate = Bounds::new(
            center.x - size.width / 2.0 - GAP / 2.0,
            center.y - size.height / 2.0 - GAP / 2.0,
            size.width + GAP,
            size.height + GAP,
        );
        if obstacles.iter().any(|shape| shape.intersects(&candidate)) {
            center.y += 100.0;
        } else {
            break;
        }
    }
    center
}

/// A free spot right of `after`, on the plane that draws it. Successive calls
/// with the same anchor stack vertically (branches off a gateway).
pub fn spot_after(doc: &Document, after: &BpmnId, size: Size) -> Result<(Scope, Point), String> {
    let (_, anchor) = doc
        .node_outline(after)
        .ok_or_else(|| format!("no element with id {after}"))?;
    let scope = scope_of_shape(doc, after).unwrap_or_default();
    let dx = STEP.max(anchor.width / 2.0 + size.width / 2.0 + GAP);
    let start = Point::new(anchor.center().x + dx, anchor.center().y);
    let center = clear_of(&obstacles(doc, &scope), start, size);
    Ok((scope, center))
}

/// A free spot inside `container` (a subprocess) or, without one, right of
/// the root content. Collapsed subprocesses must already own their plane
/// (`ensure_subprocess_plane`) before calling this.
pub fn spot_in(
    doc: &Document,
    container: Option<&BpmnId>,
    size: Size,
) -> Result<(Scope, Point), String> {
    let Some(container) = container else {
        // Free space right of everything at the root.
        let center = doc.content_bounds_in(&Scope::Root).map_or(ORIGIN, |b| {
            Point::new(b.x + b.width + GAP + size.width / 2.0, b.center().y)
        });
        return Ok((Scope::Root, center));
    };

    let Some(FlowElement::Activity(activity)) = doc.flow_element_index().get(container).copied()
    else {
        return Err(format!("{container} is not a subprocess"));
    };
    if !matches!(activity.kind, ActivityKind::SubProcess(_)) {
        return Err(format!("{container} is not a subprocess"));
    }

    let own_scope = Scope::Subprocess(container.clone());
    if doc.plane_index(&own_scope).is_some() {
        // Collapsed (or at least plane-backed): below the plane's content.
        let center = doc.content_bounds_in(&own_scope).map_or(ORIGIN, |b| {
            Point::new(
                b.x + size.width / 2.0,
                b.y + b.height + GAP + size.height / 2.0,
            )
        });
        return Ok((own_scope, center));
    }

    // Expanded: children sit on the plane that draws the subprocess's shape,
    // inside its bounds. The center must stay inside the shape — containment
    // is decided by the point.
    let (_, shape) = doc
        .node_outline(container)
        .ok_or_else(|| format!("{container} has no shape"))?;
    let scope = scope_of_shape(doc, container).unwrap_or_default();
    let children: Vec<Bounds> = {
        let index = doc.flow_element_index();
        let members: std::collections::HashSet<BpmnId> =
            doc.descendants_of(container).into_iter().collect();
        doc.plane_in(&scope)
            .map(|plane| {
                plane
                    .elements
                    .iter()
                    .filter_map(|element| match element {
                        DiElement::Shape(s)
                            if members.contains(&s.bpmn_element)
                                && index.contains_key(&s.bpmn_element) =>
                        {
                            Some(s.bounds)
                        }
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    let raw = children
        .iter()
        .copied()
        .reduce(|a, b| a.union(b))
        .map_or(shape.center(), |b| {
            Point::new(
                b.x + size.width / 2.0,
                b.y + b.height + GAP + size.height / 2.0,
            )
        });
    let center = Point::new(
        raw.x.clamp(
            shape.x + size.width / 2.0 + 10.0,
            (shape.x + shape.width - size.width / 2.0 - 10.0)
                .max(shape.x + size.width / 2.0 + 10.0),
        ),
        raw.y.clamp(
            shape.y + size.height / 2.0 + 10.0,
            (shape.y + shape.height - size.height / 2.0 - 10.0)
                .max(shape.y + size.height / 2.0 + 10.0),
        ),
    );
    Ok((scope, center))
}

/// The horizontal step from one chain node to the next.
pub fn chain_step(previous: Size, next: Size) -> f64 {
    STEP.max(previous.width / 2.0 + next.width / 2.0 + GAP)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bpmn_core::NodeKind;
    use bpmn_core::model::TaskType;

    fn task(doc: &mut Document, x: f64, y: f64) -> BpmnId {
        doc.create_node(NodeKind::Task(TaskType::Generic), Point::new(x, y))
            .expect("task created")
    }

    #[test]
    fn after_steps_right_and_nudges_down_on_collision() {
        let mut doc = Document::new();
        let a = task(&mut doc, 200.0, 150.0);
        let blocker = task(&mut doc, 350.0, 150.0);

        let (scope, spot) =
            spot_after(&doc, &a, NodeKind::Task(TaskType::Generic).default_size()).unwrap();
        assert_eq!(scope, Scope::Root);
        // The straight spot is taken by `blocker`, so the candidate drops.
        assert!((spot.x - 350.0).abs() < f64::EPSILON);
        assert!(spot.y > 150.0, "nudged below the blocker");

        let _ = blocker;
    }

    #[test]
    fn after_widens_the_step_for_wide_anchors() {
        let mut doc = Document::new();
        let sub = doc
            .create_node(
                NodeKind::SubProcess(bpmn_core::model::SubProcessKind::Embedded),
                Point::new(300.0, 200.0),
            )
            .unwrap();
        let size = NodeKind::Task(TaskType::Generic).default_size();
        let (_, spot) = spot_after(&doc, &sub, size).unwrap();
        // 350-wide subprocess: half of it plus half a task plus the gap
        // exceeds the default 150 step.
        assert!(spot.x - 300.0 > 150.0);
    }

    #[test]
    fn free_spot_lands_right_of_content_or_at_origin() {
        let doc = Document::new();
        let size = NodeKind::StartEvent.default_size();
        let (_, spot) = spot_in(&doc, None, size).unwrap();
        assert_eq!((spot.x, spot.y), (ORIGIN.x, ORIGIN.y));

        let mut doc = Document::new();
        let _ = task(&mut doc, 200.0, 150.0);
        let (_, spot) = spot_in(&doc, None, size).unwrap();
        assert!(spot.x > 250.0, "right of the task's shape");
    }

    #[test]
    fn expanded_subprocess_spot_stays_inside_the_shape() {
        let mut doc = Document::new();
        let sub = doc
            .create_node(
                NodeKind::SubProcess(bpmn_core::model::SubProcessKind::Embedded),
                Point::new(300.0, 200.0),
            )
            .unwrap();
        let size = NodeKind::Task(TaskType::Generic).default_size();
        let (scope, spot) = spot_in(&doc, Some(&sub), size).unwrap();
        assert_eq!(scope, Scope::Root);
        let (_, shape) = doc.node_outline(&sub).unwrap();
        assert!(shape.contains(spot), "center inside the subprocess shape");
    }
}
