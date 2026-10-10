//! BPMN namespace URIs.

/// The BPMN 2.0 semantic model namespace.
pub const MODEL: &str = "http://www.omg.org/spec/BPMN/20100524/MODEL";
/// The BPMN DI namespace (`BPMNDiagram`, `BPMNShape`, ...).
pub const BPMNDI: &str = "http://www.omg.org/spec/BPMN/20100524/DI";
/// The Diagram Common namespace (`dc:Bounds`, `dc:Point`).
pub const DC: &str = "http://www.omg.org/spec/DD/20100524/DC";
/// The Diagram Definition namespace (`di:waypoint`).
pub const DI: &str = "http://www.omg.org/spec/DD/20100524/DI";

/// The standard URIs paired with the conventional prefix the writer falls
/// back to when a document does not already declare one.
pub const STANDARD: [(&str, &str); 4] =
    [("bpmn", MODEL), ("bpmndi", BPMNDI), ("dc", DC), ("di", DI)];
