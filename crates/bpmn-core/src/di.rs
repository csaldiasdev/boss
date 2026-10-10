//! BPMN DI (Diagram Interchange): the visual half of a `.bpmn` file.
//!
//! DI references semantic elements by id and never the other way around, so an
//! execution engine can ignore this module entirely.

use crate::geometry::{Bounds, Point};
use crate::ids::BpmnId;
use crate::raw::RawXml;

/// A `bpmndi:BPMNDiagram`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BpmnDiagram {
    /// The `id` attribute.
    pub id: Option<BpmnId>,
    /// The `name` attribute.
    pub name: Option<String>,
    /// The diagram's plane.
    pub plane: Option<BpmnPlane>,
    /// Attributes the model does not understand, as written.
    pub unknown_attrs: Vec<(String, String)>,
    /// Children other than the plane (label styles, ...), preserved verbatim.
    pub unknown_children: Vec<RawXml>,
}

/// A `bpmndi:BPMNPlane`: the canvas for one process or collaboration.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BpmnPlane {
    /// The `id` attribute.
    pub id: Option<BpmnId>,
    /// The `bpmnElement` attribute: the process/collaboration drawn here.
    pub bpmn_element: Option<BpmnId>,
    /// Attributes the model does not understand, as written.
    pub unknown_attrs: Vec<(String, String)>,
    /// Shapes and edges in document order.
    pub elements: Vec<DiElement>,
}

/// One child of a plane.
#[derive(Debug, Clone, PartialEq)]
pub enum DiElement {
    /// A `bpmndi:BPMNShape`.
    Shape(BpmnShape),
    /// A `bpmndi:BPMNEdge`.
    Edge(BpmnEdge),
    /// Anything else, preserved verbatim.
    Unknown(RawXml),
}

/// A `bpmndi:BPMNShape`: where one flow node is drawn.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BpmnShape {
    /// The `id` attribute.
    pub id: Option<BpmnId>,
    /// The `bpmnElement` attribute: the semantic element this shape draws.
    pub bpmn_element: BpmnId,
    /// The `dc:Bounds` child.
    pub bounds: Bounds,
    /// The `isMarkerVisible` attribute (exclusive gateway X marker).
    pub is_marker_visible: Option<bool>,
    /// The `isExpanded` attribute (subprocesses).
    pub is_expanded: Option<bool>,
    /// The `isHorizontal` attribute (pools/lanes).
    pub is_horizontal: Option<bool>,
    /// The label geometry, when present.
    pub label: Option<BpmnLabel>,
    /// Attributes the model does not understand, as written.
    pub unknown_attrs: Vec<(String, String)>,
    /// Children the model does not understand, preserved verbatim.
    pub unknown_children: Vec<RawXml>,
}

/// A `bpmndi:BPMNEdge`: the polyline of one flow.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BpmnEdge {
    /// The `id` attribute.
    pub id: Option<BpmnId>,
    /// The `bpmnElement` attribute: the semantic flow this edge draws.
    pub bpmn_element: BpmnId,
    /// The `di:waypoint` children, in order.
    pub waypoints: Vec<Point>,
    /// The label geometry, when present.
    pub label: Option<BpmnLabel>,
    /// Attributes the model does not understand, as written.
    pub unknown_attrs: Vec<(String, String)>,
    /// Children the model does not understand, preserved verbatim.
    pub unknown_children: Vec<RawXml>,
}

/// A `bpmndi:BPMNLabel`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BpmnLabel {
    /// The `id` attribute.
    pub id: Option<BpmnId>,
    /// The `dc:Bounds` child, when present.
    pub bounds: Option<Bounds>,
}
