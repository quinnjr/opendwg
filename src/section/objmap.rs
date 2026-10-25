//! The object map: the `AcDb:Handles` section (ODS chapter 23).
//!
//! A run of pages, each:
//!
//! ```text
//! RS size (big-endian, counting itself) | pairs ... | RS CRC (big-endian)
//! ```
//!
//! Each pair is an unsigned `MC` handle offset and a signed `MC` location
//! offset from the previous pair. A page of size 2 (no pairs) ends the map;
//! bytes after it are ignored. ODS sets the running handle and location to 0
//! once, before the first page; every corpus file restarts both at 0 on
//! each page (the second page's first handle offset is the full handle, and
//! every location then lands on an object with a valid CRC). The CRC is the ODS 2.14.1
//! CRC-16 with seed 0xC0C1 over the page from its size through its last pair
//! (checked on every corpus page), stored big-endian as ODS says. ODS limits
//! a page to 2032 bytes; some AutoCAD files have 2034-byte pages, which are
//! read. The encoder keeps to 2032, which reproduces the R2018 samples.
//!
//! Locations are file offsets in R2000 and offsets into `AcDb:AcDbObjects`
//! from R2004; the map itself does not say which.

use crate::bits::BitReader;
use crate::codec::crc16;
use crate::diag::{Diagnostic, Severity};
use crate::error::{Error, Result};
use crate::handle::Handle;

/// `(handle, location)` pairs in file order.
pub type ObjectMap = Vec<(Handle, u64)>;

/// The largest page ODS allows, size field included.
pub const MAX_PAGE: usize = 2032;

fn corrupt(msg: &str) -> Error {
    Error::Corrupt(format!("object map: {msg}"))
}

/// A value that cannot be encoded (see [`Error::Encode`]).
fn encode_err(msg: &str) -> Error {
    Error::Encode(format!("object map: {msg}"))
}

fn be16(bytes: &[u8], at: usize) -> Result<u16> {
    bytes
        .get(at..at.saturating_add(2))
        .map(|b| u16::from_be_bytes([b[0], b[1]]))
        .ok_or(Error::Truncated {
            context: "object map",
        })
}

/// Decode the object map into `(handle, location)` pairs in file order.
/// Duplicate handles are returned as stored; the caller decides. Any bad
/// page is an error; see [`decode_with_diagnostics`] for a lenient read.
pub fn decode(bytes: &[u8]) -> Result<ObjectMap> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    while let Some(next) = decode_page(bytes, pos, &mut out)? {
        pos = next;
    }
    Ok(out)
}

/// Like [`decode`], but a bad page after at least one good one ends the map
/// with a warning: the pairs of the good pages are returned (a partial
/// drawing rather than none). A bad first page is still an error.
pub fn decode_with_diagnostics(bytes: &[u8]) -> Result<(ObjectMap, Vec<Diagnostic>)> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    let mut pages = 0usize;
    loop {
        let mut page = Vec::new();
        match decode_page(bytes, pos, &mut page) {
            Ok(Some(next)) => {
                out.append(&mut page);
                pos = next;
                pages += 1;
            }
            Ok(None) => return Ok((out, Vec::new())),
            Err(e) if pages > 0 => {
                let warning = Diagnostic {
                    severity: Severity::Warning,
                    handle: None,
                    message: format!(
                        "object map: page {} at byte {pos} unreadable ({e}); the objects \
                         of later pages are missing",
                        pages + 1
                    ),
                };
                return Ok((out, vec![warning]));
            }
            Err(e) => return Err(e),
        }
    }
}

/// Decode the page at `pos`, appending its pairs to `out`. Returns the
/// position of the next page, or `None` for the end page.
fn decode_page(bytes: &[u8], pos: usize, out: &mut Vec<(Handle, u64)>) -> Result<Option<usize>> {
    let size = usize::from(be16(bytes, pos)?);
    if size < 2 {
        return Err(corrupt(&format!("page size {size} below 2")));
    }
    let end = pos.checked_add(size).ok_or_else(|| corrupt("page size"))?;
    let stored = be16(bytes, end)?;
    let page = &bytes[pos..end]; // in range: the CRC after it was read
    if crc16(super::CRC_SEED, page) != stored {
        return Err(Error::BadCrc {
            context: "object map",
        });
    }
    if size == 2 {
        return Ok(None);
    }
    let mut r = BitReader::new(&page[2..]);
    let (mut handle, mut loc) = (0u64, 0i64);
    while r.remaining_bits() > 0 {
        let dh = r.read_umc()?;
        let dl = r.read_mc()?;
        handle = handle
            .checked_add(dh)
            .ok_or_else(|| corrupt("handle overflow"))?;
        loc = loc
            .checked_add(dl)
            .filter(|&l| l >= 0)
            .ok_or_else(|| corrupt("negative or overflowing location"))?;
        out.push((Handle(handle), loc as u64));
    }
    Ok(Some(end + 2))
}

/// Encode `(handle, location)` pairs as an object map, sorted by handle, in
/// pages of at most [`MAX_PAGE`] bytes, each restarting the running handle
/// and location at 0, then the empty end page. Fails only for a location of
/// 2^62 or more, which no modular char can hold.
pub fn encode(map: &[(Handle, u64)]) -> Result<Vec<u8>> {
    let mut sorted = map.to_vec();
    sorted.sort_by_key(|&(h, _)| h);
    let mut out = Vec::new();
    let mut page = Vec::new();
    let mut pair = Vec::with_capacity(20);
    let (mut last_h, mut last_l) = (0u64, 0i64);
    for &(h, loc) in &sorted {
        let loc = i64::try_from(loc).map_err(|_| encode_err("location too large"))?;
        pair.clear();
        encode_pair(&mut pair, h.0 - last_h, loc - last_l)?;
        if 2 + page.len() + pair.len() > MAX_PAGE {
            flush(&mut out, &page);
            page.clear();
            // The running handle and location restart at 0 on a new page.
            pair.clear();
            encode_pair(&mut pair, h.0, loc)?;
        }
        page.extend_from_slice(&pair);
        (last_h, last_l) = (h.0, loc);
    }
    if !page.is_empty() {
        flush(&mut out, &page);
    }
    flush(&mut out, &[]);
    Ok(out)
}

/// Append a pair: the handle offset as an unsigned modular char, the
/// location offset as a modular char (as `BitWriter::write_umc` and
/// `write_mc` write them, byte-aligned here).
fn encode_pair(out: &mut Vec<u8>, mut dh: u64, dl: i64) -> Result<()> {
    while dh >= 0x80 {
        out.push((dh & 0x7F) as u8 | 0x80);
        dh >>= 7;
    }
    out.push(dh as u8);
    let mut m = dl.unsigned_abs();
    if m >= 1 << 62 {
        return Err(encode_err("modular char magnitude must be below 2^62"));
    }
    while m >= 0x40 {
        out.push((m & 0x7F) as u8 | 0x80);
        m >>= 7;
    }
    out.push(m as u8 | if dl < 0 { 0x40 } else { 0 });
    Ok(())
}

/// Append one page: size, `pairs`, CRC.
fn flush(out: &mut Vec<u8>, pairs: &[u8]) {
    let start = out.len();
    // At most MAX_PAGE, so it fits in a u16.
    out.extend_from_slice(&((pairs.len() + 2) as u16).to_be_bytes());
    out.extend_from_slice(pairs);
    let crc = crc16(super::CRC_SEED, &out[start..]);
    out.extend_from_slice(&crc.to_be_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(pairs: &[(u64, i64)]) -> Vec<u8> {
        let mut body = Vec::new();
        for &(h, l) in pairs {
            encode_pair(&mut body, h, l).unwrap();
        }
        let mut out = Vec::new();
        flush(&mut out, &body);
        out
    }

    fn end_page() -> Vec<u8> {
        page(&[])
    }

    #[test]
    fn roundtrip_and_page_split() {
        // Enough entries to need several pages.
        let map: Vec<(Handle, u64)> = (1..3000u64)
            .map(|i| (Handle(i * 3), 100 + i * 1000))
            .collect();
        let bytes = encode(&map).unwrap();
        let mut pos = 0;
        let mut pages = 0;
        loop {
            let size = usize::from(u16::from_be_bytes([bytes[pos], bytes[pos + 1]]));
            assert!(size <= MAX_PAGE);
            pages += 1;
            pos += size + 2;
            if size == 2 {
                break;
            }
        }
        assert_eq!(pos, bytes.len());
        assert!(pages > 3, "{pages} pages");
        assert_eq!(decode(&bytes).unwrap(), map);
    }

    /// The byte encoder writes what the bit writer's UMC and MC write.
    #[test]
    fn pairs_match_the_bit_writer() {
        for (h, l) in [
            (0u64, 0i64),
            (1, -1),
            (0x7F, 0x3F),
            (0x80, 0x40),
            (0x3FFF, -0x40),
            (u64::MAX, (1 << 62) - 1),
            (12345, -(1 << 40)),
        ] {
            let mut w = crate::bits::BitWriter::new();
            w.write_umc(h);
            w.write_mc(l).unwrap();
            let mut got = Vec::new();
            encode_pair(&mut got, h, l).unwrap();
            assert_eq!(got, w.into_bytes(), "{h:#x} {l:#x}");
        }
        assert!(encode_pair(&mut Vec::new(), 0, 1 << 62).is_err());
    }

    #[test]
    fn encode_sorts_by_handle() {
        let map = [(Handle(9), 50), (Handle(2), 70)];
        assert_eq!(
            decode(&encode(&map).unwrap()).unwrap(),
            [(Handle(2), 70), (Handle(9), 50)]
        );
    }

    #[test]
    fn running_values_restart_per_page() {
        let mut bytes = page(&[(5, 100), (1, 40)]);
        bytes.extend(page(&[(8, 30)]));
        bytes.extend(end_page());
        assert_eq!(
            decode(&bytes).unwrap(),
            [(Handle(5), 100), (Handle(6), 140), (Handle(8), 30)]
        );
    }

    #[test]
    fn negative_location_offsets() {
        let mut bytes = page(&[(5, 100), (1, -60)]);
        bytes.extend(end_page());
        assert_eq!(decode(&bytes).unwrap(), [(Handle(5), 100), (Handle(6), 40)]);
    }

    #[test]
    fn hostile_truncated() {
        let mut bytes = page(&[(5, 100), (1, 40)]);
        bytes.extend(end_page());
        for n in 0..bytes.len() {
            assert!(decode(&bytes[..n]).is_err(), "prefix {n}");
        }
    }

    #[test]
    fn hostile_bad_crc() {
        let mut bytes = page(&[(5, 100)]);
        bytes.extend(end_page());
        bytes[2] ^= 1;
        assert!(matches!(decode(&bytes), Err(Error::BadCrc { .. })));
        let mut bytes = page(&[(5, 100)]);
        let mut end = end_page();
        end[3] ^= 1;
        bytes.extend(end);
        assert!(matches!(decode(&bytes), Err(Error::BadCrc { .. })));
    }

    #[test]
    fn bad_later_page_keeps_earlier_pages_with_a_warning() {
        let mut bytes = page(&[(5, 100), (1, 40)]);
        let mut second = page(&[(8, 30)]);
        second[2] ^= 1;
        bytes.extend(second);
        bytes.extend(end_page());
        assert!(matches!(decode(&bytes), Err(Error::BadCrc { .. })));
        let (map, diags) = decode_with_diagnostics(&bytes).unwrap();
        assert_eq!(map, [(Handle(5), 100), (Handle(6), 140)]);
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("page 2"), "{}", diags[0].message);
        // Truncated after the first page: also partial.
        let first = page(&[(5, 100)]);
        let (map, diags) = decode_with_diagnostics(&first).unwrap();
        assert_eq!((map.len(), diags.len()), (1, 1));
        // A bad first page is an error.
        let mut bytes = page(&[(5, 100)]);
        bytes[2] ^= 1;
        bytes.extend(end_page());
        assert!(decode_with_diagnostics(&bytes).is_err());
        // A good map has no warning.
        let mut bytes = page(&[(5, 100)]);
        bytes.extend(end_page());
        assert_eq!(decode_with_diagnostics(&bytes).unwrap().1, []);
    }

    #[test]
    fn hostile_absurd_size() {
        // A page size far past the data: an error, not a huge read.
        let bytes = [0xFF, 0xFF, 0, 0];
        assert!(matches!(decode(&bytes), Err(Error::Truncated { .. })));
        let bytes = [0, 1, 0, 0];
        assert!(matches!(decode(&bytes), Err(Error::Corrupt(_))));
    }

    #[test]
    fn hostile_negative_location_and_cut_pair() {
        let mut bytes = page(&[(5, -1)]);
        bytes.extend(end_page());
        assert!(matches!(decode(&bytes), Err(Error::Corrupt(_))));
        // A page ending inside a modular char.
        let mut body = Vec::new();
        encode_pair(&mut body, 5, 100).unwrap();
        body.push(0x85); // continuation with no next byte
        let mut bytes = Vec::new();
        flush(&mut bytes, &body);
        bytes.extend(end_page());
        assert!(decode(&bytes).is_err());
    }

    #[test]
    fn duplicate_handles_are_returned_as_stored() {
        let mut bytes = page(&[(5, 100), (0, 40)]);
        bytes.extend(page(&[(5, 300)]));
        bytes.extend(end_page());
        assert_eq!(
            decode(&bytes).unwrap(),
            [(Handle(5), 100), (Handle(5), 140), (Handle(5), 300)]
        );
    }

    #[test]
    fn trailing_bytes_after_end_page_are_ignored() {
        let mut bytes = page(&[(5, 100)]);
        bytes.extend(end_page());
        bytes.extend([0; 10]);
        assert_eq!(decode(&bytes).unwrap(), [(Handle(5), 100)]);
    }

    #[test]
    fn encode_rejects_huge_location() {
        assert!(encode(&[(Handle(1), 1 << 62)]).is_err());
    }
}
