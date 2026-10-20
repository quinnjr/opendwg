//! Bit-level decoding of the DWG primitive types (ODS chapter 2).

mod color;
mod reader;
mod writer;

pub(crate) use color::cmc_from_parts;
pub use color::{ACI_RGB, Color, nearest_aci};
pub use reader::BitReader;
pub use writer::BitWriter;
pub(crate) use writer::cmc_parts;

/// Test helper: pack a string of `0`/`1` characters MSB-first (spaces are
/// ignored); the last byte is zero-padded.
#[cfg(test)]
pub(crate) fn bits(s: &str) -> Vec<u8> {
    let mut out = Vec::new();
    for (n, c) in s.chars().filter(|c| !c.is_whitespace()).enumerate() {
        if n % 8 == 0 {
            out.push(0);
        }
        if c == '1' {
            *out.last_mut().unwrap() |= 0x80 >> (n % 8);
        } else {
            assert_eq!(c, '0', "bad bit char");
        }
    }
    out
}

/// Test helper: render bytes as a bit string.
#[cfg(test)]
pub(crate) fn byte_bits(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:08b}")).collect()
}
