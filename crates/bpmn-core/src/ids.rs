//! Element identifiers.

use smol_str::SmolStr;
use std::fmt;

/// A BPMN element id.
///
/// Ids are the literal XML `id` attributes (`NCName` strings such as
/// `Activity_1x2y3z`) and must round-trip byte-identical, because every
/// cross-reference in BPMN XML (`sourceRef`, `bpmnElement`, `attachedToRef`,
/// ...) is one of these strings.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BpmnId(SmolStr);

impl BpmnId {
    /// Wraps an existing id, typically read from a `.bpmn` file.
    pub fn new(id: impl Into<SmolStr>) -> Self {
        Self(id.into())
    }

    /// The id as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for BpmnId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for BpmnId {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_verbatim() {
        let id = BpmnId::new("Activity_1x2y3z");
        assert_eq!(id.as_str(), "Activity_1x2y3z");
        assert_eq!(id.to_string(), "Activity_1x2y3z");
    }
}
