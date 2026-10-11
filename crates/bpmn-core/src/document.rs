//! The in-memory document: semantic model + DI, with lookup helpers.

use crate::di::{BpmnDiagram, DiElement};
use crate::geometry::Bounds;
use crate::ids::BpmnId;
use crate::model::{ActivityKind, Definitions, FlowElement, Process, RootElement};
use std::collections::{HashMap, HashSet};

/// A loaded `.bpmn` document.
///
/// `Default` (in `edit.rs`) is a fresh document with one empty process, not an
/// all-empty struct.
#[derive(Debug, Clone, PartialEq)]
pub struct Document {
    /// The document root.
    pub definitions: Definitions,
}

impl Document {
    /// All processes, in document order.
    pub fn processes(&self) -> impl Iterator<Item = &Process> {
        self.definitions
            .root_elements
            .iter()
            .filter_map(|root| match root {
                RootElement::Process(process) => Some(process),
                RootElement::Unknown(_) => None,
            })
    }

    /// All flow elements across all processes, in document order, descending
    /// into subprocesses (a subprocess is yielded before its children).
    pub fn flow_elements(&self) -> impl Iterator<Item = &FlowElement> {
        let mut stack: Vec<_> = self.processes().map(|p| p.flow_elements.iter()).collect();
        stack.reverse(); // the stack pops from the back: first process on top
        FlowElements { stack }
    }

    /// The subprocess id that directly contains `id`, or the containing
    /// process id for top-level elements. `None` if `id` is not in the
    /// document.
    pub fn container_of(&self, id: &BpmnId) -> Option<BpmnId> {
        fn scan(container: &BpmnId, elements: &[FlowElement], id: &BpmnId) -> Option<BpmnId> {
            for element in elements {
                if element.id() == Some(id) {
                    return Some(container.clone());
                }
                if let FlowElement::Activity(activity) = element
                    && let ActivityKind::SubProcess(sub) = &activity.kind
                    && let Some(found) = scan(&activity.node.base.id, &sub.flow_elements, id)
                {
                    return Some(found);
                }
            }
            None
        }
        self.processes()
            .find_map(|process| scan(&process.base.id, &process.flow_elements, id))
    }

    /// Every element id nested (at any depth) inside the subprocess `id`.
    /// Empty when `id` is not a subprocess.
    pub fn descendants_of(&self, id: &BpmnId) -> Vec<BpmnId> {
        fn collect(elements: &[FlowElement], out: &mut Vec<BpmnId>) {
            for element in elements {
                if let Some(id) = element.id() {
                    out.push(id.clone());
                }
                if let FlowElement::Activity(activity) = element
                    && let ActivityKind::SubProcess(sub) = &activity.kind
                {
                    collect(&sub.flow_elements, out);
                }
            }
        }
        let mut out = Vec::new();
        if let Some(FlowElement::Activity(activity)) = self.flow_element_index().get(id)
            && let ActivityKind::SubProcess(sub) = &activity.kind
        {
            collect(&sub.flow_elements, &mut out);
        }
        out
    }

    /// Ids hidden on the canvas because an ancestor subprocess is collapsed
    /// (its DI shape does not carry `isExpanded="true"`).
    pub fn hidden_by_collapse(&self) -> HashSet<BpmnId> {
        let mut hidden = HashSet::new();
        let Some(plane) = self.diagram().and_then(|d| d.plane.as_ref()) else {
            return hidden;
        };
        for element in &plane.elements {
            let DiElement::Shape(shape) = element else {
                continue;
            };
            if shape.is_expanded == Some(true) || hidden.contains(&shape.bpmn_element) {
                continue;
            }
            hidden.extend(self.descendants_of(&shape.bpmn_element));
        }
        hidden
    }

    /// An id → flow-element lookup table (rebuild after mutations).
    pub fn flow_element_index(&self) -> HashMap<&BpmnId, &FlowElement> {
        self.flow_elements()
            .filter_map(|element| element.id().map(|id| (id, element)))
            .collect()
    }

    /// The first diagram, which is what the canvas renders.
    pub fn diagram(&self) -> Option<&BpmnDiagram> {
        self.definitions.diagrams.first()
    }

    /// The bounding box of everything the first diagram draws, in world
    /// coordinates. Drives zoom-to-fit when a file opens.
    pub fn content_bounds(&self) -> Option<Bounds> {
        let plane = self.diagram()?.plane.as_ref()?;

        let mut min_x = f64::INFINITY;
        let mut min_y = f64::INFINITY;
        let mut max_x = f64::NEG_INFINITY;
        let mut max_y = f64::NEG_INFINITY;
        let mut any = false;

        let mut extend = |x: f64, y: f64| {
            min_x = min_x.min(x);
            min_y = min_y.min(y);
            max_x = max_x.max(x);
            max_y = max_y.max(y);
            any = true;
        };

        for element in &plane.elements {
            match element {
                DiElement::Shape(shape) => {
                    let b = shape.bounds;
                    extend(b.x, b.y);
                    extend(b.x + b.width, b.y + b.height);
                }
                DiElement::Edge(edge) => {
                    for point in &edge.waypoints {
                        extend(point.x, point.y);
                    }
                }
                DiElement::Unknown(_) => {}
            }
        }

        any.then(|| Bounds::new(min_x, min_y, max_x - min_x, max_y - min_y))
    }
}

/// Depth-first walk over flow elements, descending into subprocesses.
struct FlowElements<'a> {
    stack: Vec<std::slice::Iter<'a, FlowElement>>,
}

impl<'a> Iterator for FlowElements<'a> {
    type Item = &'a FlowElement;

    fn next(&mut self) -> Option<&'a FlowElement> {
        loop {
            let iter = self.stack.last_mut()?;
            match iter.next() {
                Some(element) => {
                    if let FlowElement::Activity(activity) = element
                        && let ActivityKind::SubProcess(sub) = &activity.kind
                    {
                        self.stack.push(sub.flow_elements.iter());
                    }
                    return Some(element);
                }
                None => {
                    self.stack.pop();
                }
            }
        }
    }
}
