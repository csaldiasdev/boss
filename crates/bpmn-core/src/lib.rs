//! BPMN 2.0 semantic model and diagram interchange (DI) types.
//!
//! This crate holds the process model (flow nodes, flows, collaborations) and
//! the DI geometry (shapes, edges, waypoints) as plain data with no UI or XML
//! dependencies, so that the editor, the XML layer, the validator, and a future
//! execution engine can all share it.

pub mod di;
pub mod document;
pub mod geometry;
pub mod ids;
pub mod model;
pub mod raw;

pub use document::Document;
pub use geometry::{Bounds, Point, Size};
pub use ids::BpmnId;
pub use raw::RawXml;
