//! Verbatim XML preservation.

/// A verbatim XML fragment preserved for round-trip fidelity.
///
/// Anything the model does not (yet) understand — vendor extensions such as
/// `camunda:*`/`zeebe:*`, `extensionElements` content, or BPMN elements outside
/// the modeled subset — is captured as the exact source text and re-emitted
/// unchanged on write. Policy: never drop what we don't understand.
///
/// Fragments may reference namespace prefixes declared on the document root;
/// the writer therefore re-emits every original root declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawXml(pub String);

impl RawXml {
    /// The fragment source text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
