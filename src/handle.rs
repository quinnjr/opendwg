//! Object handles and handle references.

use std::fmt;

/// A unique object handle. `Handle(0)` is the null handle (the default).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Handle(pub u64);

impl fmt::Display for Handle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:X}", self.0)
    }
}

/// A handle reference as stored in the file: a code plus a value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct HandleRef {
    /// The reference code (ODS 2.13): 2-5 absolute (soft/hard owner/pointer), 6, 8, 0xA, 0xC offsets from the referring object's handle.
    pub code: u8,
    /// The handle, or the offset.
    pub value: u64,
}

impl HandleRef {
    /// Resolve against `base` per the ODS handle-reference codes.
    /// Returns `None` for unknown codes and on overflow/underflow.
    pub fn resolve(self, base: Handle) -> Option<Handle> {
        match self.code {
            2..=5 => Some(Handle(self.value)),
            6 => base.0.checked_add(1).map(Handle),
            8 => base.0.checked_sub(1).map(Handle),
            0xA => base.0.checked_add(self.value).map(Handle),
            0xC => base.0.checked_sub(self.value).map(Handle),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handle_ref_offsets() {
        let b = Handle(0x10);
        assert_eq!(
            HandleRef { code: 6, value: 0 }.resolve(b),
            Some(Handle(0x11))
        );
        assert_eq!(
            HandleRef { code: 8, value: 0 }.resolve(b),
            Some(Handle(0x0F))
        );
        assert_eq!(
            HandleRef {
                code: 0xA,
                value: 5
            }
            .resolve(b),
            Some(Handle(0x15))
        );
        assert_eq!(
            HandleRef {
                code: 0xC,
                value: 5
            }
            .resolve(b),
            Some(Handle(0x0B))
        );
        assert_eq!(
            HandleRef {
                code: 5,
                value: 0x2A
            }
            .resolve(b),
            Some(Handle(0x2A))
        );
        assert_eq!(
            HandleRef {
                code: 0xC,
                value: 0x20
            }
            .resolve(b),
            None
        );
    }

    #[test]
    fn handle_overflow_and_display() {
        assert_eq!(
            HandleRef { code: 6, value: 0 }.resolve(Handle(u64::MAX)),
            None
        );
        assert_eq!(HandleRef { code: 8, value: 0 }.resolve(Handle(0)), None);
        assert_eq!(HandleRef { code: 0, value: 1 }.resolve(Handle(1)), None);
        assert_eq!(Handle(0xAB).to_string(), "AB");
    }
}
