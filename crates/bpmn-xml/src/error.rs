//! Parse errors.

/// Why a `.bpmn` file could not be parsed.
#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    /// The underlying XML is malformed.
    #[error("malformed XML near byte {position}: {source}")]
    Xml {
        /// Byte offset where the reader stopped.
        position: u64,
        /// The quick-xml error.
        source: quick_xml::Error,
    },
    /// The XML is well-formed but not the BPMN structure we expect.
    #[error("unexpected BPMN structure: {0}")]
    Structure(String),
    /// The root element is not `bpmn:definitions`.
    #[error("not a BPMN file: no <definitions> root element found")]
    MissingDefinitions,
}
