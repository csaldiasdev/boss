//! The in-memory document: semantic model + DI, with lookup helpers.

use crate::di::{BpmnDiagram, DiElement};
use crate::geometry::Bounds;
use crate::ids::BpmnId;
use crate::model::{Definitions, FlowElement, Process, RootElement};
use std::collections::HashMap;

/// A loaded `.bpmn` document.
#[derive(Debug, Clone, PartialEq, Default)]
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

    /// All flow elements across all processes, in document order.
    pub fn flow_elements(&self) -> impl Iterator<Item = &FlowElement> {
        self.processes().flat_map(|p| p.flow_elements.iter())
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
