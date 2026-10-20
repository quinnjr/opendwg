//! Diagnostics produced while reading or validating a drawing.

use std::fmt;

use crate::handle::Handle;

/// How serious a [`Diagnostic`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub enum Severity {
    /// Something was kept, repaired or left out; the drawing is usable.
    Warning,
    /// Something is broken: an object could not be read, or the drawing cannot be written as it is.
    Error,
}

/// A single problem report, optionally tied to an object handle.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Diagnostic {
    /// How serious it is.
    pub severity: Severity,
    /// The object it is about, if any.
    pub handle: Option<Handle>,
    /// What happened.
    pub message: String,
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Severity::Warning => "warning",
            Severity::Error => "error",
        })
    }
}

/// `error at 2A: message`, or `warning: message` for a diagnostic of no
/// object.
impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.handle {
            Some(h) => write!(f, "{} at {h}: {}", self.severity, self.message),
            None => write!(f, "{}: {}", self.severity, self.message),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display() {
        let d = Diagnostic {
            severity: Severity::Warning,
            handle: Some(Handle(0x1F)),
            message: "dangling".into(),
        };
        assert_eq!(d.to_string(), "warning at 1F: dangling");
        let d = Diagnostic {
            severity: Severity::Error,
            handle: None,
            message: "no layer 0".into(),
        };
        assert_eq!(d.to_string(), "error: no layer 0");
        assert_eq!(Severity::Error.to_string(), "error");
    }
}
