//! Validation rule engine for BPMN 2.0 documents.
//!
//! Headless (no UI dependencies) so the editor, a CLI linter, and the MCP
//! server can all run the same rules. Implemented in milestone M4.

// Referenced now so the dependency graph is wired from M0; used in earnest in M4.
pub use bpmn_core::BpmnId;
