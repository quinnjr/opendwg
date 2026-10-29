//! The crate's [`Error`] type and [`Result`] alias.

use crate::diag::Diagnostic;
use crate::handle::Handle;

/// Everything that can go wrong reading, editing or writing a drawing.
///
/// Reading fails when the container (file header, section maps), the header
/// section, the classes section or the object map is broken (see
/// [`crate::Drawing::from_bytes`]); objects that fail to decode are kept
/// raw, and problems with them become [`crate::Diagnostic`]s instead. Writing fails on objects AutoCAD 2018
/// cannot store ([`Error::UnconvertibleObjects`]), on a drawing
/// [`crate::Drawing::validate`] finds errors in ([`Error::Invalid`]) and on
/// values a field cannot hold ([`Error::Encode`]). The editing methods
/// refuse changes that would break the drawing ([`Error::InUse`],
/// [`Error::Protected`], [`Error::NotFound`], [`Error::Invalid`]).
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The data ends before a structure it must hold: a file cut short.
    #[error("truncated data while reading {context}")]
    Truncated {
        /// What was being read.
        context: &'static str,
    },
    /// Not a DWG version this crate reads (AC1015 to AC1032), or not a DWG
    /// file: the version magic found.
    #[error("unsupported DWG version: {0}")]
    UnsupportedVersion(String),
    /// A password-protected drawing: the named required section is
    /// encrypted. Encrypted drawings are not supported.
    #[error("password-encrypted drawings are not supported (section {0} is encrypted)")]
    Encrypted(String),
    /// A section's start or end sentinel (fixed marker bytes) is wrong:
    /// which one.
    #[error("bad sentinel: {0}")]
    BadSentinel(&'static str),
    /// A checksum of the container does not match.
    #[error("CRC mismatch in {context}")]
    BadCrc {
        /// What the checksum covers.
        context: &'static str,
    },
    /// Data read from a file is inconsistent (a count past the data, an
    /// unknown code, ...): what is wrong.
    #[error("corrupt data: {0}")]
    Corrupt(String),
    /// A value the writer cannot encode (a string too long for its field, a
    /// count too large, a value R2018 has no form for, ...): the drawing
    /// (or the value given to an encoder) cannot be saved as it is. Never
    /// returned for data read from a file, which is [`Error::Corrupt`].
    #[error("cannot encode: {0}")]
    Encode(String),
    /// Reading or writing a file failed.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// Objects AutoCAD 2018 cannot store (see
    /// [`crate::Drawing::unconvertible_objects`]): each one's handle and
    /// type name. Nothing was written; remove them (see
    /// [`crate::Drawing::remove_all`]) to save.
    #[error("{} object(s) cannot be converted: {}", .0.len(), first_objects(.0))]
    UnconvertibleObjects(Vec<(Handle, String)>),
    /// An object cannot be removed while these objects or header variables
    /// (by name, e.g. `"$CLAYER"`) refer to it.
    #[error(
        "still in use by {} object(s) and {} header variable(s)",
        .objects.len(),
        .header_vars.len()
    )]
    InUse {
        /// The objects that still refer to it, in handle order.
        objects: Vec<Handle>,
        /// The header variables that name it.
        header_vars: Vec<&'static str>,
    },
    /// An object the drawing cannot do without (layer `0`, model space, ...):
    /// what it is and why it is kept.
    #[error("protected: {0}")]
    Protected(String),
    /// No object has this handle.
    #[error("object not found: {0}")]
    NotFound(Handle),
    /// The drawing, or the change asked for, is invalid: the error
    /// diagnostics saying why (from [`crate::Drawing::validate`] or the
    /// editing method); from a save, preceded by the warnings of the
    /// required objects the writer could not add, saying why.
    #[error("drawing is invalid: {} diagnostic(s): {}", .0.len(), first_diagnostics(.0))]
    Invalid(Vec<Diagnostic>),
}

/// `Result` with this crate's [`Error`].
pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    /// The variant's name (`"InUse"`, `"Truncated"`, ...).
    pub fn kind(&self) -> &'static str {
        match self {
            Error::Truncated { .. } => "Truncated",
            Error::UnsupportedVersion(_) => "UnsupportedVersion",
            Error::Encrypted(_) => "Encrypted",
            Error::BadSentinel(_) => "BadSentinel",
            Error::BadCrc { .. } => "BadCrc",
            Error::Corrupt(_) => "Corrupt",
            Error::Encode(_) => "Encode",
            Error::Io(_) => "Io",
            Error::UnconvertibleObjects(_) => "UnconvertibleObjects",
            Error::InUse { .. } => "InUse",
            Error::Protected(_) => "Protected",
            Error::NotFound(_) => "NotFound",
            Error::Invalid(_) => "Invalid",
        }
    }

    /// The objects the error is about: the unconvertible objects, the
    /// objects still using one, the one not found, the objects of the
    /// diagnostics; `None` for errors about no object (an [`Error::InUse`]
    /// by header variables only, an [`Error::Invalid`] whose diagnostics
    /// name no object, and the other kinds).
    pub fn handles(&self) -> Option<Vec<Handle>> {
        let handles: Vec<Handle> = match self {
            Error::UnconvertibleObjects(list) => list.iter().map(|&(h, _)| h).collect(),
            Error::InUse { objects, .. } => objects.clone(),
            Error::NotFound(h) => vec![*h],
            Error::Invalid(diags) => diags.iter().filter_map(|d| d.handle).collect(),
            _ => return None,
        };
        (!handles.is_empty()).then_some(handles)
    }
}

/// With the `serde` feature: `{ "kind": ..., "message": ..., "handles": [...] }`
/// ([`Error::kind`], the `Display` message, and [`Error::handles`] when
/// there are any), for a front end (a Tauri command can return the error as
/// is). Not deserializable.
#[cfg(feature = "serde")]
impl serde::Serialize for Error {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let handles = self.handles();
        let mut st = s.serialize_struct("Error", 2 + usize::from(handles.is_some()))?;
        st.serialize_field("kind", self.kind())?;
        st.serialize_field("message", &self.to_string())?;
        if let Some(h) = &handles {
            st.serialize_field("handles", h)?;
        } else {
            st.skip_field("handles")?;
        }
        st.end()
    }
}

/// How many entries of a list an error message shows.
const SHOWN: usize = 3;

/// The first [`SHOWN`] items, joined with `sep`, and how many more there are.
fn first<T>(items: &[T], sep: &str, show: impl Fn(&T) -> String) -> String {
    let shown: Vec<String> = items.iter().take(SHOWN).map(show).collect();
    let mut out = shown.join(sep);
    if items.len() > SHOWN {
        out.push_str(&format!(" and {} more", items.len() - SHOWN));
    }
    out
}

fn first_objects(objects: &[(Handle, String)]) -> String {
    first(objects, ", ", |(h, class)| format!("{class} {h}"))
}

fn first_diagnostics(diagnostics: &[Diagnostic]) -> String {
    first(diagnostics, "; ", Diagnostic::to_string)
}

/// An [`Error::Invalid`] with one error diagnostic, of no object.
pub(crate) fn invalid(message: impl Into<String>) -> Error {
    Error::Invalid(vec![Diagnostic {
        severity: crate::diag::Severity::Error,
        handle: None,
        message: message.into(),
    }])
}

/// An [`Error::Encode`].
pub(crate) fn encode_error(msg: impl Into<String>) -> Error {
    Error::Encode(msg.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::diag::Severity;

    #[test]
    fn messages_show_the_first_entries() {
        let objects: Vec<(Handle, String)> = (0..5)
            .map(|i| (Handle(0x90 + i), format!("CLASS{i}")))
            .collect();
        assert_eq!(
            Error::UnconvertibleObjects(objects[..2].to_vec()).to_string(),
            "2 object(s) cannot be converted: CLASS0 90, CLASS1 91"
        );
        assert_eq!(
            Error::UnconvertibleObjects(objects).to_string(),
            "5 object(s) cannot be converted: CLASS0 90, CLASS1 91, CLASS2 92 and 2 more"
        );
        let diag = |h: Option<u64>, m: &str| Diagnostic {
            severity: Severity::Error,
            handle: h.map(Handle),
            message: m.into(),
        };
        assert_eq!(
            Error::Invalid(vec![
                diag(Some(0x2A), "bad owner"),
                diag(None, "no layer 0")
            ])
            .to_string(),
            "drawing is invalid: 2 diagnostic(s): error at 2A: bad owner; error: no layer 0"
        );
        let many: Vec<Diagnostic> = (0..4).map(|i| diag(Some(i), "x")).collect();
        assert_eq!(
            Error::Invalid(many).to_string(),
            "drawing is invalid: 4 diagnostic(s): error at 0: x; error at 1: x; error at 2: x \
             and 1 more"
        );
        assert_eq!(
            Error::Encode("too long".into()).to_string(),
            "cannot encode: too long"
        );
    }

    #[test]
    fn types_are_send_sync() {
        fn a<T: Send + Sync>() {}
        a::<Handle>();
        a::<Error>();
    }
}
