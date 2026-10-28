//! R2000 (AC1015) file reader, ODS chapter 3 (R13-R15 file layout).
//!
//! How each section is located:
//! - Header, Classes, Handles come from the section-locator records in the file header
//!   (record 0, 1, 2: absolute seeker + size). Header and Classes slices include their
//!   begin and end sentinels; the object map has none.
//! - Template: locator record 4 (the MEASUREMENT variable), when its size is nonzero.
//! - AuxHeader: locator record 5, accepted only when the data starts with `FF 77 01`
//!   (ODS chapter 27). ODS calls record 5 "unknown"; ACadSharp-written files point it here.
//! - Preview: the image seeker at 0x0D points at the start sentinel; the slice runs through
//!   the end sentinel (ODS 14.2). A malformed preview is skipped with a warning, not an error.
//! - Objects: R2000 object-map offsets are absolute, so the whole file is stored as
//!   `Objects` and `objects_base` is 0. This copies the whole file (memory bounded by the
//!   input size; a known cost).
//! - Optional sections that are absent (size or seeker 0) are skipped. A Template, record 5
//!   or Preview that is out of range, or a Preview without its sentinels, is skipped with a
//!   warning (a truncated file loses its last section this way). Record 5 data in range but
//!   without the AuxHeader signature is not an AuxHeader and is skipped without a warning,
//!   since ODS gives record 5 no meaning. Required sections (Header, Classes, Handles) are
//!   errors.
//! - ObjFreeSpace has no locator in R2000, so it is never produced here.

use std::collections::BTreeMap;

use super::{RawSections, SectionName, u32_at};
use crate::codec::crc16;
use crate::error::{Error, Result};
use crate::version::Version;

const HEADER_START: [u8; 16] = [
    0xCF, 0x7B, 0x1F, 0x23, 0xFD, 0xDE, 0x38, 0xA9, 0x5F, 0x7C, 0x68, 0xB8, 0x4E, 0x6D, 0x33, 0x5F,
];
const HEADER_END: [u8; 16] = [
    0x30, 0x84, 0xE0, 0xDC, 0x02, 0x21, 0xC7, 0x56, 0xA0, 0x83, 0x97, 0x47, 0xB1, 0x92, 0xCC, 0xA0,
];
const CLASSES_START: [u8; 16] = [
    0x8D, 0xA1, 0xC4, 0xB8, 0xC4, 0xA9, 0xF8, 0xC5, 0xC0, 0xDC, 0xF4, 0x5F, 0xE7, 0xCF, 0xB6, 0x8A,
];
const CLASSES_END: [u8; 16] = [
    0x72, 0x5E, 0x3B, 0x47, 0x3B, 0x56, 0x07, 0x3A, 0x3F, 0x23, 0x0B, 0xA0, 0x18, 0x30, 0x49, 0x75,
];
/// Follows the file-header CRC.
const FILE_HEADER_END: [u8; 16] = [
    0x95, 0xA0, 0x4E, 0x28, 0x99, 0x82, 0x1A, 0xE5, 0x5E, 0x41, 0xE0, 0x5F, 0x9D, 0x3A, 0x4D, 0x00,
];
const PREVIEW_START: [u8; 16] = [
    0x1F, 0x25, 0x6D, 0x07, 0xD4, 0x36, 0x28, 0x28, 0x9D, 0x57, 0xCA, 0x3F, 0x9D, 0x44, 0x10, 0x2B,
];
const PREVIEW_END: [u8; 16] = [
    0xE0, 0xDA, 0x92, 0xF8, 0x2B, 0xC9, 0xD7, 0xD7, 0x62, 0xA8, 0x35, 0xC0, 0x62, 0xBB, 0xEF, 0xD4,
];

const OFF_MAINTENANCE: usize = 0x0B;
const OFF_IMAGE_SEEKER: usize = 0x0D;
const OFF_CODEPAGE: usize = 0x13;
const OFF_LOCATOR_COUNT: usize = 0x15;
const OFF_LOCATORS: usize = 0x19;
const LOCATOR_LEN: usize = 9;

/// The bytes `[seeker, seeker + size)`, or `None` when out of range.
fn span(bytes: &[u8], seeker: u32, size: u32) -> Option<&[u8]> {
    let start = usize::try_from(seeker).ok()?;
    let end = start.checked_add(usize::try_from(size).ok()?)?;
    bytes.get(start..end)
}

fn check_sentinels(
    slice: &[u8],
    start: &[u8; 16],
    end: &[u8; 16],
    what: &'static str,
) -> Result<()> {
    if slice.len() < 32 {
        return Err(Error::Truncated { context: what });
    }
    if slice[..16] != start[..] || slice[slice.len() - 16..] != end[..] {
        return Err(Error::BadSentinel(what));
    }
    Ok(())
}

pub(super) fn read(bytes: &[u8]) -> Result<RawSections> {
    let maintenance = *bytes.get(OFF_MAINTENANCE).ok_or(Error::Truncated {
        context: "file header",
    })?;
    let codepage = bytes
        .get(OFF_CODEPAGE..OFF_CODEPAGE + 2)
        .and_then(|s| <[u8; 2]>::try_from(s).ok())
        .map(u16::from_le_bytes)
        .ok_or(Error::Truncated {
            context: "file header",
        })?;
    let count = u32_at(bytes, OFF_LOCATOR_COUNT, "section locator count")?;

    // ODS 3.2.6: the CRC is XORed with a constant chosen by the record count (3 to 6).
    let xor = match count {
        3 => 0xA598,
        4 => 0x8101,
        5 => 0x3CC4,
        6 => 0x8461,
        _ => {
            return Err(Error::Corrupt(format!(
                "unsupported section locator count {count}"
            )));
        }
    };
    let count = count as usize; // 3..=6
    let crc_at = OFF_LOCATORS + count * LOCATOR_LEN;
    let crc_bytes = bytes.get(crc_at..crc_at + 2).ok_or(Error::Truncated {
        context: "file header CRC",
    })?;
    if bytes.get(crc_at + 2..crc_at + 18) != Some(&FILE_HEADER_END[..]) {
        return Err(if bytes.len() < crc_at + 18 {
            Error::Truncated {
                context: "file header sentinel",
            }
        } else {
            Error::BadSentinel("file header end")
        });
    }
    let stored = u16::from_le_bytes([crc_bytes[0], crc_bytes[1]]);
    if crc16(0, &bytes[..crc_at]) ^ xor != stored {
        return Err(Error::BadCrc {
            context: "file header",
        });
    }

    let mut locators: [Option<(u32, u32)>; 6] = [None; 6];
    for i in 0..count {
        let at = OFF_LOCATORS + i * LOCATOR_LEN;
        let record = bytes[at] as usize; // in range: crc_at was found past it
        let seeker = u32_at(bytes, at + 1, "section locator")?;
        let size = u32_at(bytes, at + 5, "section locator")?;
        if let Some(slot) = locators.get_mut(record) {
            *slot = Some((seeker, size));
        }
    }
    let required = |idx: usize, what: &'static str| -> Result<&[u8]> {
        let (seeker, size) =
            locators[idx].ok_or(Error::Corrupt(format!("missing locator: {what}")))?;
        span(bytes, seeker, size).ok_or(Error::Truncated { context: what })
    };

    let mut sections = BTreeMap::new();

    let header = required(0, "header variables")?;
    check_sentinels(header, &HEADER_START, &HEADER_END, "header variables")?;
    sections.insert(SectionName::Header, header.to_vec());

    let classes = required(1, "classes")?;
    check_sentinels(classes, &CLASSES_START, &CLASSES_END, "classes")?;
    sections.insert(SectionName::Classes, classes.to_vec());

    sections.insert(SectionName::Handles, required(2, "object map")?.to_vec());

    // Optional sections (Template, AuxHeader, Preview): absent ones (size or seeker 0: offset 0
    // is the file header itself) are skipped; present ones that are out of range (or a preview without its sentinels) are
    // skipped with a warning.
    // Locator record 4 is stored as `Template`: ODS 3.2.6 says it holds only the MEASUREMENT
    // variable, which ODS places in the AcDb:Template section (chapter 22).
    let mut warnings = Vec::new();
    let present = |&(seeker, size): &(u32, u32)| seeker != 0 && size > 0;
    if let Some((seeker, size)) = locators[4].filter(present) {
        match span(bytes, seeker, size) {
            Some(t) => {
                sections.insert(SectionName::Template, t.to_vec());
            }
            None => warnings.push(format!(
                "locator record 4 (template) at {seeker} size {size} is out of range; skipped"
            )),
        }
    }
    if let Some((seeker, size)) = locators[5].filter(present) {
        match span(bytes, seeker, size) {
            // ODS calls record 5 unknown; it is the AuxHeader only with its signature.
            Some(a) if a.starts_with(&[0xFF, 0x77, 0x01]) => {
                sections.insert(SectionName::AuxHeader, a.to_vec());
            }
            Some(_) => {}
            None => warnings.push(format!(
                "locator record 5 at {seeker} size {size} is out of range; skipped"
            )),
        }
    }

    // Preview: start sentinel, RL overall size, area, end sentinel (ODS 14.2).
    let mut preview_offset = None;
    let seeker = u32_at(bytes, OFF_IMAGE_SEEKER, "image seeker")?;
    if seeker != 0 {
        let preview = usize::try_from(seeker)
            .ok()
            .and_then(|s| bytes.get(s..))
            .filter(|rest| rest.starts_with(&PREVIEW_START))
            .and_then(|rest| {
                let area = u32_at(rest, 16, "preview size").ok()?;
                let total = usize::try_from(area).ok()?.checked_add(36)?;
                rest.get(..total).filter(|p| p.ends_with(&PREVIEW_END))
            });
        match preview {
            Some(p) => {
                preview_offset = Some(u64::from(seeker));
                sections.insert(SectionName::Preview, p.to_vec());
            }
            None => warnings.push(format!(
                "preview at {seeker} is out of range or malformed; skipped"
            )),
        }
    }

    // R2000 object-map offsets are absolute file offsets.
    sections.insert(SectionName::Objects, bytes.to_vec());

    Ok(RawSections {
        version: Version::R2000,
        maintenance,
        codepage,
        sections,
        objects_base: 0,
        preview_offset,
        section_meta: BTreeMap::new(),
        file_header: None,
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Synthetic file: header with `locs` (record, seeker, size), a valid CRC for the real
    /// locator count and end sentinel, then `body`. `count` overrides the stored count.
    fn build(locs: &[(u8, u32, u32)], count: Option<u32>, image: u32, body: &[u8]) -> Vec<u8> {
        let mut b = vec![0u8; OFF_LOCATORS];
        b[..6].copy_from_slice(b"AC1015");
        b[OFF_IMAGE_SEEKER..OFF_IMAGE_SEEKER + 4].copy_from_slice(&image.to_le_bytes());
        let n = count.unwrap_or(locs.len() as u32);
        b[OFF_LOCATOR_COUNT..OFF_LOCATOR_COUNT + 4].copy_from_slice(&n.to_le_bytes());
        for &(r, s, z) in locs {
            b.push(r);
            b.extend_from_slice(&s.to_le_bytes());
            b.extend_from_slice(&z.to_le_bytes());
        }
        let xor = match locs.len() {
            3 => 0xA598,
            4 => 0x8101,
            5 => 0x3CC4,
            _ => 0x8461,
        };
        let crc = crc16(0, &b) ^ xor;
        b.extend_from_slice(&crc.to_le_bytes());
        b.extend_from_slice(&FILE_HEADER_END);
        b.extend_from_slice(body);
        b
    }

    /// Body with a valid header and classes block at known offsets; returns (file, locs).
    fn valid(header_start: [u8; 16]) -> (usize, Vec<u8>) {
        let base = OFF_LOCATORS + 3 * LOCATOR_LEN + 2 + 16;
        let mut body = Vec::new();
        body.extend_from_slice(&header_start);
        body.extend_from_slice(&HEADER_END);
        body.extend_from_slice(&CLASSES_START);
        body.extend_from_slice(&CLASSES_END);
        body.extend_from_slice(&[0, 0]); // object map
        (base, body)
    }

    fn locs(base: usize) -> Vec<(u8, u32, u32)> {
        let b = base as u32;
        vec![(0, b, 32), (1, b + 32, 32), (2, b + 64, 2)]
    }

    #[test]
    fn synthetic_valid_file_reads() {
        let (base, body) = valid(HEADER_START);
        let raw = read(&build(&locs(base), None, 0, &body)).unwrap();
        assert_eq!(raw.sections[&SectionName::Handles], [0, 0]);
    }

    #[test]
    fn wrong_header_sentinel() {
        let (base, body) = valid([0; 16]);
        let r = read(&build(&locs(base), None, 0, &body));
        assert!(matches!(r, Err(Error::BadSentinel(_))));
    }

    #[test]
    fn hostile_locators_error() {
        let (base, body) = valid(HEADER_START);
        let ok = locs(base);
        let cases: Vec<Vec<(u8, u32, u32)>> = vec![
            vec![(0, u32::MAX, 32), ok[1], ok[2]],
            vec![ok[0], (1, u32::MAX - 8, 32), ok[2]],
            vec![ok[0], ok[1], (2, base as u32 + 64, u32::MAX)],
            vec![ok[0], ok[1], (2, u32::MAX, u32::MAX)],
            vec![(0, base as u32, 31), ok[1], ok[2]],
            vec![(0, base as u32, 0), ok[1], ok[2]],
            vec![(0, base as u32, 10_000), ok[1], ok[2]],
        ];
        for c in cases {
            assert!(read(&build(&c, None, 0, &body)).is_err(), "{c:?}");
        }
    }

    #[test]
    fn bad_locator_count() {
        let (base, body) = valid(HEADER_START);
        for n in [0, 1, 2, 7, 1000, u32::MAX] {
            let r = read(&build(&locs(base), Some(n), 0, &body));
            assert!(r.is_err(), "count {n}");
        }
    }

    #[test]
    fn huge_preview_size_is_skipped_not_panic() {
        let (base, mut body) = valid(HEADER_START);
        let at = (base + body.len()) as u32;
        body.extend_from_slice(&PREVIEW_START);
        body.extend_from_slice(&u32::MAX.to_le_bytes());
        body.extend_from_slice(&PREVIEW_END);
        let raw = read(&build(&locs(base), None, at, &body)).unwrap();
        assert!(raw.preview_offset.is_none());
        assert!(!raw.sections.contains_key(&SectionName::Preview));
        assert_eq!(raw.warnings.len(), 1, "{:?}", raw.warnings);
    }

    #[test]
    fn out_of_range_optional_locators_are_skipped() {
        let (_, body) = valid(HEADER_START);
        let base = OFF_LOCATORS + 5 * LOCATOR_LEN + 2 + 16;
        let mut l = locs(base);
        l.push((4, u32::MAX, 4));
        l.push((5, u32::MAX - 1, u32::MAX));
        let raw = read(&build(&l, None, u32::MAX, &body)).unwrap();
        assert!(!raw.sections.contains_key(&SectionName::Template));
        assert!(!raw.sections.contains_key(&SectionName::AuxHeader));
        // Template, record 5 and the preview each add a warning.
        assert_eq!(raw.warnings.len(), 3, "{:?}", raw.warnings);
    }

    /// An optional locator at offset 0 with a size is absent: the file's own
    /// header bytes are not read as the Template section (whose MEASUREMENT
    /// they would set) nor as record 5.
    #[test]
    fn optional_locators_at_offset_zero_are_absent() {
        let (_, body) = valid(HEADER_START);
        let base = OFF_LOCATORS + 5 * LOCATOR_LEN + 2 + 16;
        let mut l = locs(base);
        l.push((4, 0, 6));
        l.push((5, 0, 6));
        let raw = read(&build(&l, None, 0, &body)).unwrap();
        assert!(!raw.sections.contains_key(&SectionName::Template));
        assert!(!raw.sections.contains_key(&SectionName::AuxHeader));
        assert!(raw.warnings.is_empty(), "{:?}", raw.warnings);
    }

    #[test]
    fn absent_optional_sections_add_no_warning() {
        let (base, body) = valid(HEADER_START);
        let raw = read(&build(&locs(base), None, 0, &body)).unwrap();
        assert!(raw.warnings.is_empty(), "{:?}", raw.warnings);
    }
}
