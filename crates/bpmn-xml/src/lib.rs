//! BPMN 2.0 XML (ISO/IEC 19510) reader and writer.
//!
//! Hand-written on top of `quick-xml` (serde cannot express BPMN's
//! substitution groups, namespace handling, and unknown-element passthrough).
//! Unknown and vendor-extension content is preserved verbatim so files
//! round-trip cleanly with Camunda Modeler, bpmn.io, and friends.

mod error;
pub mod ns;
mod read;
mod write;

pub use error::ParseError;
pub use read::parse;
pub use write::write;
