//! BPMN 2.0 XML (ISO/IEC 19510) reader and writer.
//!
//! Hand-written on top of `quick-xml` (serde cannot express BPMN's
//! substitution groups, namespace handling, and unknown-element passthrough).
//! Unknown and vendor-extension content is preserved verbatim so files
//! round-trip cleanly with Camunda Modeler, bpmn.io, and friends.
//!
//! Implemented in milestone M1.

// Referenced now so the dependency graph is wired from M0; used in earnest in M1.
pub use bpmn_core::BpmnId;
