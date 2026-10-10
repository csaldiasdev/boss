//! MCP server exposing the boss editor to AI agents.
//!
//! Serves the Model Context Protocol over streamable HTTP on localhost so an
//! agent can inspect and edit the open document while the user watches. Talks
//! to the editor exclusively through a channel-based handle (single writer:
//! the app's update loop), never through shared mutable state.
//!
//! Implemented in milestone M5 (desktop only).

// Referenced now so the dependency graph is wired from M0; used in earnest in M5.
pub use bpmn_core::BpmnId;
