//! R2007 file reader (AC1021), ODS chapter 5.
//!
//! Stages:
//! 1. [`decode_file_header`]: the 0x400-byte page at 0x80 (ODS 5.2). The first 3 * 255 bytes are
//!    three interleaved RS(255,239) codewords. The decoded data holds a check sequence, the
//!    compressed header data and their CRCs; the header data decompresses to 0x110 bytes.
//! 2. [`read_system_page`]: the pages map, then the sections map (ODS 5.2, 5.3). Each is
//!    RS(255,239)-decoded with a block count derived from its repeat count (`CorrectionFactor`),
//!    checked against its two CRCs, and decompressed.
//! 3. [`read_section`]: each data page (ODS 5.4) is RS(255,251)-decoded (encoding 4) or taken as
//!    is (encoding 1), checked, decompressed and placed at its offset, with omitted zero pages
//!    restored.
//!
//! Page addresses in the pages map are relative to 0x480, the end of the file header page.
//!
//! Integrity checks (all confirmed on the corpus, see `tests/container.rs`):
//! - File header: the check sequence CRC (normal CRC64, `UpdateSeed1`), the compressed data CRC
//!   and the header data CRC (normal CRC64, `UpdateSeed2`), seed 0 (ODS 5.2.1).
//! - System pages: mirrored CRC64 over the compressed and the decompressed data, seed
//!   `UpdateSeed1(CrcSeed)` (ODS 5.3).
//! - Data pages: mirrored CRC64 over the compressed data, seed `UpdateSeed1(CrcSeed)`, and the
//!   ODS 5.4.1 checksum over the decompressed data, seed `CrcSeed`. The section map stores both.
//!
//! RS(255,251) corrects at most 2 bytes per codeword and can miscorrect beyond that; the page CRC
//! is checked after decoding, so a miscorrection shows up as a CRC mismatch.
//!
//! The 0x28 bytes of check data at the end of the file header page (ODS 5.2.1.1.5) are derived
//! from random values only and protect nothing the reader uses; they are not checked.
//!
//! Policies:
//! - A file header or system page CRC mismatch is `Err(Error::BadCrc)`. A data page CRC or
//!   checksum mismatch is a warning in [`RawSections::warnings`] and the page is still used, as
//!   for R2004 data pages.
//! - A file header's file size other than the input's is a warning; larger, the file was cut
//!   short, and with a section that cannot be read that is `Truncated`. A header copy at `Header2offset` that is missing, fails to decode or differs is a
//!   warning; but missing while a section cannot be read, the file was cut short
//!   (`Truncated`).
//! - Header, Classes, Handles and Objects are required; other sections that fail to read are
//!   skipped with a warning. Unknown names are kept as [`SectionName::Other`], including the
//!   empty-named last section.
//! - Sections with encryption 1 (password-protected) are rejected with `Error::Encrypted`.
//! - `objects_base` is 0; `preview_offset` is the meta data's preview address when nonzero and
//!   inside the file (the start of the raw preview page in the corpus).
//!
//! Where the corpus disagrees with ODS:
//! - The section map's `SectionNameLength` is the name's length in bytes, including the 2-byte
//!   terminator (ODS: "SectionNameLength x 2 [+ 2]").
//! - Data pages may decompress to more than the section's `Max size` (0xF800): `AcDb:AcDbObjects`
//!   pages reach 0x11380 bytes in the corpus.
//! - Encoding 1 pages are stored without RS parity (ODS 5.4 says non-interleaved RS).
//!
//! Allocation and work bounds (every size from the file is checked before it is used):
//! - RS decoding only reads blocks that lie inside the file, and a system page decompresses to
//!   at most the file size.
//! - Pages-map entries must be a nonzero multiple of 0x20 bytes (ODS 5.4: pages start on 0x20
//!   boundaries). Each page may be used by one section only, and not at all if it is the pages
//!   map or section map, so every page is decoded at most once.
//! - Section names are at most 0x100 bytes, so each warning is short and their total stays
//!   linear in the number of pages.
//! - A compressed data page decompresses to at most 0x1F000 bytes (twice the largest page size
//!   ODS lists). A stored page holds at most its own file bytes.
//! - All sections together produce at most one budget: per in-file page, 0x1F000 bytes (or the
//!   page's size, if larger) for pages of at least 0x100 bytes (anything RS-encoded needs one
//!   255-byte codeword), or the page's size for smaller pages, plus the file size (for omitted
//!   zero pages). That is at most about 500 times the input size.

use std::collections::{BTreeMap, BTreeSet};

use super::{RawSections, SectionName, check_required, is_required, span, u32_at, u64_at};
use crate::codec::{
    crc64, crc64_mirrored, crc64_seed1, crc64_seed2, lz77_2007, page_checksum_2007, rs2007,
};
use crate::error::{Error, Result};
use crate::version::Version;

const OFF_MAINTENANCE: usize = 0x0B;
const OFF_PREVIEW: usize = 0x0D;
const OFF_CODEPAGE: usize = 0x13;
const FILE_HEADER_AT: u64 = 0x80;
/// Page-map addresses are relative to the end of the 0x400-byte file header page (ODS 5.2).
const PAGE_BASE: u64 = 0x480;
const CODEWORD: usize = 255;
/// RS data bytes per codeword: file header and system pages (ODS 5.13).
const SYSTEM_K: usize = 239;
/// RS data bytes per codeword: data pages (ODS 5.13).
const DATA_K: usize = 251;
const HEADER_BLOCKS: usize = 3;
/// Decompressed file header data size (ODS 5.2).
const HEADER_DATA_LEN: usize = 0x110;
/// Check sequence CRC, key, compressed data CRC, compressed length, length 2 (ODS 5.2).
const HEADER_PREFIX_LEN: usize = 0x20;
const HEADER_CRC_AT: usize = 0x108;
/// Largest decompressed size of one compressed data page.
const PAGE_OUTPUT_MAX: u64 = 0x1F000;
/// Smallest page that can hold an RS codeword (255 bytes, rounded up to 0x20).
const MIN_ENCODED_PAGE: u64 = 0x100;
const SECTION_DESC_LEN: usize = 0x40;
const PAGE_REF_LEN: usize = 0x38;
/// Longest section name accepted, in bytes (UTF-16 with terminator). ODS names are under 30
/// characters; the cap keeps names, which are repeated in warnings, small.
const MAX_NAME_LEN: u64 = 0x100;

/// Where a system page is and how it was encoded (ODS 5.2 file header fields).
#[derive(Debug, Clone, PartialEq, Eq)]
struct SystemPage {
    compressed: u64,
    uncompressed: u64,
    /// `CorrectionFactor`: how many times the aligned data is repeated before RS encoding.
    repeat: u64,
    crc_compressed: u64,
    crc_uncompressed: u64,
}

/// The fields of the decoded R2007 file header data that the reader uses.
#[derive(Debug, Clone, PartialEq, Eq)]
struct R2007FileHeader {
    file_size: u64,
    /// Pages map address relative to [`PAGE_BASE`].
    pages_map_offset: u64,
    /// The pages map's own id in the pages map.
    pages_map_id: u64,
    pages_map: SystemPage,
    /// Address of the header copy, relative to [`PAGE_BASE`].
    header2_offset: u64,
    sections_map_id: u64,
    sections_map: SystemPage,
    crc_seed: u64,
    /// The decompressed 0x110 bytes, for comparison with the copy.
    data: Vec<u8>,
}

/// A page of the file, located through the pages map.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Page {
    address: u64,
    size: u64,
}

/// One stored page of a data section (ODS 5.2, section map).
#[derive(Debug, Clone, PartialEq, Eq)]
struct PageRef {
    offset: u64,
    id: u64,
    uncompressed: u64,
    compressed: u64,
    checksum: u64,
    crc: u64,
}

/// One section description from the section map (ODS 5.2).
#[derive(Debug, Clone, PartialEq, Eq)]
struct SectionDesc {
    name: String,
    size: u64,
    encrypted: u64,
    /// 1 = stored without RS parity, 4 = RS(255,251) interleaved.
    encoding: u64,
    pages: Vec<PageRef>,
}

fn corrupt(msg: String) -> Error {
    Error::Corrupt(msg)
}

/// Decode and check the file header page at `at` (ODS 5.2): RS(255,239) with 3 interleaved
/// blocks, then the three CRCs, then decompression to 0x110 bytes.
fn decode_file_header(bytes: &[u8], at: u64) -> Result<R2007FileHeader> {
    const CTX: &str = "R2007 file header";
    let page = span(bytes, at, (HEADER_BLOCKS * CODEWORD) as u64)
        .ok_or(Error::Truncated { context: CTX })?;
    let d = rs2007::decode(page, HEADER_BLOCKS, SYSTEM_K, HEADER_BLOCKS)?;

    // ODS 5.2.1.4: the check sequence is the key and the key rotated left by its low 5 bits.
    let key = u64_at(&d, 0x08, CTX)?;
    let mut seq = [0u8; 16];
    seq[..8].copy_from_slice(&key.to_le_bytes());
    seq[8..].copy_from_slice(&key.rotate_left((key & 0x1F) as u32).to_le_bytes());
    if crc64(crc64_seed1(0, seq.len()), &seq) != u64_at(&d, 0x00, CTX)? {
        return Err(Error::BadCrc {
            context: "R2007 file header check sequence",
        });
    }

    // ODS 5.2: a negative length means the data is stored uncompressed.
    let len = u32_at(&d, 0x18, CTX)? as i32;
    let stored = len < 0;
    let n = usize::try_from(len.unsigned_abs())
        .ok()
        .filter(|&n| n > 0 && (!stored || n == HEADER_DATA_LEN))
        .ok_or_else(|| corrupt(format!("{CTX}: data length {len}")))?;
    let payload = d
        .get(HEADER_PREFIX_LEN..HEADER_PREFIX_LEN + n)
        .ok_or_else(|| corrupt(format!("{CTX}: data length {len} exceeds the page")))?;
    if crc64(crc64_seed2(0, n), payload) != u64_at(&d, 0x10, CTX)? {
        return Err(Error::BadCrc {
            context: "R2007 file header compressed data",
        });
    }
    let mut data = if stored {
        payload.to_vec()
    } else {
        lz77_2007::decompress(payload, HEADER_DATA_LEN)?
    };
    let stored_crc = u64_at(&data, HEADER_CRC_AT, CTX)?;
    data[HEADER_CRC_AT..].fill(0);
    if crc64(crc64_seed2(0, HEADER_DATA_LEN), &data) != stored_crc {
        return Err(Error::BadCrc {
            context: "R2007 file header data",
        });
    }
    data[HEADER_CRC_AT..].copy_from_slice(&stored_crc.to_le_bytes());

    let f = |at: usize| u64_at(&data, at, CTX);
    Ok(R2007FileHeader {
        file_size: f(0x08)?,
        pages_map_offset: f(0x38)?,
        pages_map_id: f(0x40)?,
        pages_map: SystemPage {
            compressed: f(0x50)?,
            uncompressed: f(0x58)?,
            repeat: f(0x18)?,
            crc_compressed: f(0x10)?,
            crc_uncompressed: f(0x80)?,
        },
        header2_offset: f(0x48)?,
        sections_map_id: f(0xC0)?,
        sections_map: SystemPage {
            compressed: f(0xB0)?,
            uncompressed: f(0xC8)?,
            repeat: f(0xD8)?,
            crc_compressed: f(0xD0)?,
            crc_uncompressed: f(0xA8)?,
        },
        crc_seed: f(0xF0)?,
        data,
    })
}

/// Read the system page at file address `address` (ODS 5.3): RS(255,239)-decode
/// `ceil(aligned * repeat / 239)` interleaved blocks, check the compressed CRC, decompress when
/// the compressed size is below the uncompressed size, and check the uncompressed CRC.
/// `limit`, when given, is the page's size from the pages map, which the blocks must fit in.
fn read_system_page(
    bytes: &[u8],
    address: u64,
    limit: Option<u64>,
    sp: &SystemPage,
    crc_seed: u64,
    what: &'static str,
) -> Result<Vec<u8>> {
    let (c, u) = (sp.compressed, sp.uncompressed);
    if c == 0 || c > u || u > bytes.len() as u64 {
        return Err(corrupt(format!(
            "{what}: sizes {c:#x} (compressed) / {u:#x} out of range"
        )));
    }
    let blocks = c
        .div_ceil(8)
        .checked_mul(8)
        .and_then(|aligned| aligned.checked_mul(sp.repeat))
        .map(|total| total.div_ceil(SYSTEM_K as u64))
        .filter(|&b| b > 0)
        .ok_or_else(|| corrupt(format!("{what}: repeat count {} out of range", sp.repeat)))?;
    let encoded = blocks.saturating_mul(CODEWORD as u64);
    if limit.is_some_and(|l| encoded > l) {
        return Err(corrupt(format!(
            "{what}: {blocks} RS blocks do not fit in the page"
        )));
    }
    let src = span(bytes, address, encoded).ok_or(Error::Truncated { context: what })?;
    // `blocks` is at most the file size / 255 here.
    let blocks = blocks as usize;
    let decoded = rs2007::decode(src, blocks, SYSTEM_K, blocks)?;
    let payload = &decoded[..c as usize];
    if crc64_mirrored(crc64_seed1(crc_seed, payload.len()), payload) != sp.crc_compressed {
        return Err(Error::BadCrc { context: what });
    }
    let data = if c < u {
        lz77_2007::decompress(payload, u as usize)?
    } else {
        payload.to_vec()
    };
    if crc64_mirrored(crc64_seed1(crc_seed, data.len()), &data) != sp.crc_uncompressed {
        return Err(Error::BadCrc { context: what });
    }
    Ok(data)
}

/// Parse the decompressed pages map (ODS 5.2): `(size, id)` pairs of i64; addresses accumulate
/// from [`PAGE_BASE`]. Pages are keyed by `|id|`, as in ODS; sizes must be a nonzero multiple of
/// 0x20 and ids unique.
fn parse_pages_map(data: &[u8]) -> Result<BTreeMap<u64, Page>> {
    const CTX: &str = "R2007 pages map";
    if data.len() % 16 != 0 {
        return Err(corrupt(format!(
            "{CTX}: size {:#x} is not a whole number of entries",
            data.len()
        )));
    }
    let mut pages = BTreeMap::new();
    let mut address = PAGE_BASE;
    for e in data.chunks_exact(16) {
        let size = u64_at(e, 0, CTX)?;
        let id = (u64_at(e, 8, CTX)? as i64).unsigned_abs();
        if size == 0 || size % 0x20 != 0 {
            return Err(corrupt(format!("{CTX}: page {id} has size {size:#x}")));
        }
        if pages.insert(id, Page { address, size }).is_some() {
            return Err(corrupt(format!("{CTX}: duplicate page {id}")));
        }
        address = address
            .checked_add(size)
            .ok_or_else(|| corrupt(format!("{CTX}: address overflows")))?;
    }
    Ok(pages)
}

/// Parse the decompressed section map (ODS 5.2). Lengths are checked against the remaining
/// bytes before anything is allocated.
fn parse_section_map(data: &[u8]) -> Result<Vec<SectionDesc>> {
    const CTX: &str = "R2007 section map";
    let mut pos = 0;
    let mut descs = Vec::new();
    while pos < data.len() {
        let d = data
            .get(pos..pos + SECTION_DESC_LEN)
            .ok_or(Error::Truncated { context: CTX })?;
        pos += SECTION_DESC_LEN;
        let f = |at: usize| u64_at(d, at, CTX);
        let rest = (data.len() - pos) as u64;
        let name_len = f(0x20)?;
        if name_len > rest || name_len > MAX_NAME_LEN || name_len % 2 != 0 {
            return Err(corrupt(format!("{CTX}: name length {name_len:#x}")));
        }
        let name_bytes = &data[pos..pos + name_len as usize];
        pos += name_len as usize;
        let units: Vec<u16> = name_bytes
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        let name = String::from_utf16_lossy(&units)
            .trim_end_matches('\0')
            .to_owned();

        let page_count = f(0x38)?;
        let refs_len = page_count
            .checked_mul(PAGE_REF_LEN as u64)
            .filter(|&n| n <= rest - name_len)
            .ok_or_else(|| corrupt(format!("{CTX}: page count {page_count} exceeds the map")))?
            as usize;
        let pages = data[pos..pos + refs_len]
            .chunks_exact(PAGE_REF_LEN)
            .map(|r| {
                let g = |at: usize| u64_at(r, at, CTX);
                Ok(PageRef {
                    offset: g(0x00)?,
                    id: g(0x10)?,
                    uncompressed: g(0x18)?,
                    compressed: g(0x20)?,
                    checksum: g(0x28)?,
                    crc: g(0x30)?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        pos += refs_len;
        descs.push(SectionDesc {
            name,
            size: f(0x00)?,
            encrypted: f(0x10)?,
            encoding: f(0x30)?,
            pages,
        });
    }
    Ok(descs)
}

/// State shared by the sections of one file while they are read.
struct SectionReader<'a> {
    bytes: &'a [u8],
    pages_map: &'a BTreeMap<u64, Page>,
    crc_seed: u64,
    /// Output still allowed for the whole file; each section's size is taken from it.
    budget: u64,
    /// Page ids already used by a section.
    used_pages: BTreeSet<u64>,
    warnings: Vec<String>,
}

/// Read one data section (ODS 5.4): RS-decode (encoding 4) or take (encoding 1) each page's
/// stored bytes, check its CRC, decompress it, check its checksum and place it at its offset;
/// gaps and the tail are zero pages that were not stored.
///
/// CRC and checksum mismatches are pushed to `warnings`; everything else malformed is an error.
fn read_section(st: &mut SectionReader<'_>, desc: &SectionDesc) -> Result<Vec<u8>> {
    let name = &desc.name;
    let bad = |msg: String| corrupt(format!("section {name:?}: {msg}"));
    if desc.encrypted == 1 {
        return Err(Error::Encrypted(name.clone()));
    }
    if desc.encoding != 1 && desc.encoding != 4 {
        return Err(bad(format!("unknown encoding {}", desc.encoding)));
    }
    if desc.size > st.budget {
        return Err(bad(format!(
            "size {:#x} exceeds the output budget left for this file ({:#x})",
            desc.size, st.budget
        )));
    }
    let size = usize::try_from(desc.size)
        .map_err(|_| bad(format!("size {:#x} out of range", desc.size)))?;
    st.budget -= desc.size;

    let mut out = Vec::new();
    for r in &desc.pages {
        let (c, u) = (r.compressed, r.uncompressed);
        let end = r.offset.checked_add(u).filter(|&e| e <= desc.size);
        if r.offset < out.len() as u64 || end.is_none() {
            return Err(bad(format!(
                "page {} at offset {:#x} (size {u:#x}) out of order or past the section end",
                r.id, r.offset
            )));
        }
        if c == 0 || c > u || (c < u && u > PAGE_OUTPUT_MAX) {
            return Err(bad(format!(
                "page {}: sizes {c:#x} (compressed) / {u:#x} out of range",
                r.id
            )));
        }
        let page = *st
            .pages_map
            .get(&r.id)
            .ok_or_else(|| bad(format!("page {} not in the pages map", r.id)))?;
        if !st.used_pages.insert(r.id) {
            return Err(bad(format!(
                "page {} is already used by another section",
                r.id
            )));
        }

        let decoded;
        let payload = if desc.encoding == 4 {
            let blocks = c.div_ceil(8).saturating_mul(8).div_ceil(DATA_K as u64);
            let encoded = blocks.saturating_mul(CODEWORD as u64);
            if encoded > page.size {
                return Err(bad(format!(
                    "page {}: {blocks} RS blocks do not fit in its {:#x} bytes",
                    r.id, page.size
                )));
            }
            let src = span(st.bytes, page.address, encoded).ok_or(Error::Truncated {
                context: "R2007 data page",
            })?;
            // `blocks` is at most the page size / 255 here.
            let blocks = blocks as usize;
            decoded = rs2007::decode(src, blocks, DATA_K, blocks)?;
            &decoded[..c as usize]
        } else {
            if c > page.size {
                return Err(bad(format!(
                    "page {}: {c:#x} bytes do not fit in its {:#x} bytes",
                    r.id, page.size
                )));
            }
            span(st.bytes, page.address, c).ok_or(Error::Truncated {
                context: "R2007 data page",
            })?
        };

        if crc64_mirrored(crc64_seed1(st.crc_seed, payload.len()), payload) != r.crc {
            st.warnings.push(format!(
                "section {name:?}: page {} at {:#x} has a bad CRC",
                r.id, page.address
            ));
        }
        let data = if c < u {
            lz77_2007::decompress(payload, u as usize)?
        } else {
            payload.to_vec()
        };
        // Seed `CrcSeed` is unverified for a nonzero CrcSeed (0 in every corpus file).
        if u64::from(page_checksum_2007(st.crc_seed, &data)) != r.checksum {
            st.warnings.push(format!(
                "section {name:?}: page {} at {:#x} has a bad checksum",
                r.id, page.address
            ));
        }
        // `offset <= size`, which fits in usize.
        out.resize(r.offset as usize, 0);
        out.extend_from_slice(&data);
    }
    out.resize(size, 0);
    Ok(out)
}

pub(super) fn read(bytes: &[u8]) -> Result<RawSections> {
    match read_whole(bytes) {
        Err(Error::Corrupt(_) | Error::BadCrc { .. } | Error::BadSentinel(_))
            if cut_short(bytes) =>
        {
            Err(Error::Truncated {
                context: "R2007 file (cut short)",
            })
        }
        r => r,
    }
}

/// Whether the file is shorter than its header says, or lacks its trailing header copy: a
/// failure to read it then comes from data cut off.
fn cut_short(bytes: &[u8]) -> bool {
    let Ok(header) = decode_file_header(bytes, FILE_HEADER_AT) else {
        return false;
    };
    header.file_size > bytes.len() as u64
        || PAGE_BASE
            .checked_add(header.header2_offset)
            .is_none_or(|at| matches!(decode_file_header(bytes, at), Err(Error::Truncated { .. })))
}

/// [`read`], whose errors a cut file turns into `Truncated`.
fn read_whole(bytes: &[u8]) -> Result<RawSections> {
    let maintenance = *bytes.get(OFF_MAINTENANCE).ok_or(Error::Truncated {
        context: "file header",
    })?;
    let codepage = bytes
        .get(OFF_CODEPAGE..OFF_CODEPAGE + 2)
        .map(|s| u16::from_le_bytes([s[0], s[1]]))
        .ok_or(Error::Truncated {
            context: "file header",
        })?;
    let preview = u32_at(bytes, OFF_PREVIEW, "file header")?;
    let header = decode_file_header(bytes, FILE_HEADER_AT)?;
    let mut warnings = Vec::new();

    let len = bytes.len() as u64;
    // A file shorter than its header says is checked once the sections are read, like a
    // missing header copy: only when data is missing is it `Truncated`.
    let short = header.file_size > len;
    if header.file_size < len {
        warnings.push(format!(
            "R2007 file header gives the file size as {:#x}, the file has {len:#x} bytes",
            header.file_size
        ));
    }
    // ODS 5.2.1.8: the header page is repeated at the end of the file. A missing copy is a
    // warning when every section reads (see `copy_missing` below).
    let mut copy_missing = false;
    match PAGE_BASE
        .checked_add(header.header2_offset)
        .ok_or(Error::Truncated {
            context: "repeated R2007 file header",
        })
        .and_then(|at| decode_file_header(bytes, at))
    {
        Ok(copy) if copy.data == header.data => {}
        Ok(_) => warnings.push("repeated R2007 file header differs from the first".to_owned()),
        Err(Error::Truncated { .. }) => copy_missing = true,
        Err(e) => warnings.push(format!("repeated R2007 file header unreadable: {e}")),
    }

    let pages_map_at = PAGE_BASE
        .checked_add(header.pages_map_offset)
        .ok_or_else(|| corrupt("R2007 pages map address overflows".into()))?;
    let pages_map = parse_pages_map(&read_system_page(
        bytes,
        pages_map_at,
        None,
        &header.pages_map,
        header.crc_seed,
        "R2007 pages map",
    )?)?;
    let section_map_page = pages_map.get(&header.sections_map_id).ok_or_else(|| {
        corrupt(format!(
            "section map page {} not in the pages map",
            header.sections_map_id
        ))
    })?;
    let descs = parse_section_map(&read_system_page(
        bytes,
        section_map_page.address,
        Some(section_map_page.size),
        &header.sections_map,
        header.crc_seed,
        "R2007 section map",
    )?)?;

    let budget = pages_map
        .values()
        .filter(|p| p.address.saturating_add(p.size) <= len)
        .map(|p| {
            if p.size >= MIN_ENCODED_PAGE {
                p.size.max(PAGE_OUTPUT_MAX)
            } else {
                p.size
            }
        })
        .fold(len, u64::saturating_add);
    let mut st = SectionReader {
        bytes,
        pages_map: &pages_map,
        crc_seed: header.crc_seed,
        budget,
        // The system pages are in the pages map too; no section may use them.
        used_pages: BTreeSet::from([header.pages_map_id, header.sections_map_id]),
        warnings,
    };
    let mut skipped = false;
    let mut names_seen = BTreeSet::new();
    let mut sections = BTreeMap::new();
    for desc in &descs {
        let name = SectionName::from_r2004_name(&desc.name);
        if !names_seen.insert(name.clone()) {
            return Err(corrupt(format!(
                "section map: duplicate section {:?}",
                desc.name
            )));
        }
        match read_section(&mut st, desc) {
            Ok(data) => {
                sections.insert(name, data);
            }
            Err(e) if !is_required(&name) => {
                skipped = true;
                st.warnings
                    .push(format!("section {:?} skipped: {e}", desc.name));
            }
            Err(e) => return Err(e),
        }
    }
    check_required(&sections)?;
    if copy_missing || short {
        // Cut short (no trailing header copy, or fewer bytes than the header says) and with
        // a section unreadable, data is missing. With every section read, only the end is.
        if skipped {
            return Err(Error::Truncated {
                context: "R2007 file (cut short, and a section is missing)",
            });
        }
        if copy_missing {
            st.warnings.push(
                "repeated R2007 file header missing at the end of the file; every section was \
                 read"
                    .to_owned(),
            );
        }
        if short {
            st.warnings.push(format!(
                "R2007 file header gives the file size as {:#x}, the file has {len:#x} bytes \
                 (cut short; every section was read)",
                header.file_size
            ));
        }
    }

    let preview_offset = Some(u64::from(preview)).filter(|&p| p != 0 && p < len);
    Ok(RawSections {
        version: Version::R2007,
        maintenance,
        codepage,
        sections,
        objects_base: 0,
        preview_offset,
        section_meta: BTreeMap::new(),
        file_header: None,
        warnings: st.warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::lz77_2007::literal_stream;
    use crate::codec::rs2007::encode_interleaved;

    /// A section of the synthetic file. `page_size` splits `data` into pages; all-zero pages
    /// are omitted. Compressed sections need 8-periodic page contents (see [`repeat_stream`]).
    #[derive(Clone)]
    struct Sec {
        name: &'static str,
        data: Vec<u8>,
        encoding: u64,
        compressed: bool,
        page_size: usize,
    }

    /// A stored data page: its section-map reference, its file bytes and its pages-map id
    /// (normally `r.id`; tests may change either).
    #[derive(Clone)]
    struct DataPage {
        r: PageRef,
        file: Vec<u8>,
        map_id: i64,
    }

    #[derive(Clone)]
    struct SecModel {
        name: String,
        size: u64,
        encrypted: u64,
        encoding: u64,
        pages: Vec<DataPage>,
    }

    /// A synthetic file before serialization; tests edit it to make it hostile.
    #[derive(Clone)]
    struct Model {
        secs: Vec<SecModel>,
        /// Extra pages-map entries `(size, id)` appended after the real ones.
        extra_entries: Vec<(u64, i64)>,
        /// Header data fields overwritten before the header CRC is computed.
        header_fields: Vec<(usize, u64)>,
        /// Repeat count for system pages.
        repeat: u64,
        /// Bytes appended after the file (before the header copy).
        tail_padding: usize,
    }

    /// `unit` (8 bytes) repeated to `total` bytes as an LZ77 stream: 8 literals, then one long
    /// back-reference at distance 8 (ODS 5.10.2 opcode 0x2?, long form).
    fn repeat_stream(unit: &[u8], total: usize) -> Vec<u8> {
        assert_eq!(unit.len(), 8);
        let rest = total - 8 - 0x100;
        assert!(rest < 0x1_0000);
        let mut s = vec![0x00];
        s.extend_from_slice(unit);
        s.push(0x28 | (rest & 7) as u8);
        s.extend_from_slice(&7u16.to_le_bytes()); // distance 8 - 1, plus 1 in the long form
        s.push((rest >> 3) as u8);
        s.push((((rest >> 11) & 0x1F) << 3) as u8); // no literals follow
        s
    }

    fn pad(b: &mut Vec<u8>, to: usize) {
        b.resize(b.len().div_ceil(to) * to, 0);
    }

    fn sections() -> Vec<Sec> {
        let periodic = |unit: &[u8; 8], n: usize| unit.repeat(n / 8);
        let mut objects = periodic(b"OBJECTS!", 0x200);
        objects.extend([0u8; 0x200]); // an all-zero page, omitted from the file
        objects.extend(periodic(b"objects2", 0x180));
        vec![
            Sec {
                name: "AcDb:Header",
                data: periodic(b"HEADVARS", 0x300),
                encoding: 4,
                compressed: true,
                page_size: 0x800,
            },
            Sec {
                name: "AcDb:Classes",
                data: b"classes, stored as is".to_vec(),
                encoding: 4,
                compressed: false,
                page_size: 0xF800,
            },
            Sec {
                name: "AcDb:Handles",
                data: vec![0, 2],
                encoding: 4,
                compressed: false,
                page_size: 0xF800,
            },
            Sec {
                name: "AcDb:AcDbObjects",
                data: objects,
                encoding: 4,
                compressed: true,
                page_size: 0x200,
            },
            Sec {
                name: "AcDb:AppInfo",
                data: b"app info".to_vec(),
                encoding: 1,
                compressed: false,
                page_size: 0x300,
            },
            Sec {
                name: "AcDb:Unheard",
                data: vec![9; 3],
                encoding: 4,
                compressed: false,
                page_size: 0xF800,
            },
            Sec {
                name: "",
                data: Vec::new(),
                encoding: 4,
                compressed: false,
                page_size: 0xF800,
            },
        ]
    }

    fn model_with(secs: &[Sec]) -> Model {
        let mut id = 1u64;
        let secs = secs
            .iter()
            .map(|s| {
                let mut pages = Vec::new();
                for (p, chunk) in s.data.chunks(s.page_size).enumerate() {
                    if chunk.iter().all(|&b| b == 0) {
                        continue;
                    }
                    let payload = if s.compressed {
                        repeat_stream(&chunk[..8], chunk.len())
                    } else {
                        chunk.to_vec()
                    };
                    let mut file = if s.encoding == 4 {
                        let mut aligned = payload.clone();
                        pad(&mut aligned, 8);
                        encode_interleaved(&aligned, DATA_K)
                    } else {
                        payload.clone()
                    };
                    pad(&mut file, 0x20);
                    pages.push(DataPage {
                        r: PageRef {
                            offset: (p * s.page_size) as u64,
                            id,
                            uncompressed: chunk.len() as u64,
                            compressed: payload.len() as u64,
                            checksum: u64::from(page_checksum_2007(0, chunk)),
                            crc: crc64_mirrored(crc64_seed1(0, payload.len()), &payload),
                        },
                        file,
                        map_id: id as i64,
                    });
                    id += 1;
                }
                SecModel {
                    name: s.name.to_owned(),
                    size: s.data.len() as u64,
                    encrypted: 0,
                    encoding: s.encoding,
                    pages,
                }
            })
            .collect();
        Model {
            secs,
            extra_entries: Vec::new(),
            header_fields: Vec::new(),
            repeat: 2,
            tail_padding: 0,
        }
    }

    fn model() -> Model {
        model_with(&sections())
    }

    fn put(b: &mut Vec<u8>, v: u64) {
        b.extend_from_slice(&v.to_le_bytes());
    }

    /// A stored (uncompressed) system page: file bytes, size and both CRCs.
    fn system_page(data: &[u8], repeat: u64) -> (Vec<u8>, SystemPage) {
        let mut aligned = data.to_vec();
        pad(&mut aligned, 8);
        let mut file = encode_interleaved(&aligned.repeat(repeat as usize), SYSTEM_K);
        pad(&mut file, 0x20);
        let crc = crc64_mirrored(crc64_seed1(0, data.len()), data);
        let sp = SystemPage {
            compressed: data.len() as u64,
            uncompressed: data.len() as u64,
            repeat,
            crc_compressed: crc,
            crc_uncompressed: crc,
        };
        (file, sp)
    }

    fn section_map(m: &Model) -> Vec<u8> {
        let mut map = Vec::new();
        for s in &m.secs {
            let mut name: Vec<u8> = s.name.encode_utf16().flat_map(u16::to_le_bytes).collect();
            if !name.is_empty() {
                name.extend([0, 0]);
            }
            for v in [
                s.size,
                0xF800,
                s.encrypted,
                0,
                name.len() as u64,
                0,
                s.encoding,
            ] {
                put(&mut map, v);
            }
            put(&mut map, s.pages.len() as u64);
            map.extend(name);
            for p in &s.pages {
                let r = &p.r;
                for v in [
                    r.offset,
                    r.uncompressed,
                    r.id,
                    r.uncompressed,
                    r.compressed,
                    r.checksum,
                    r.crc,
                ] {
                    put(&mut map, v);
                }
            }
        }
        map
    }

    /// RS-encode file header data as the 0x400-byte header page.
    fn header_page(data: &[u8]) -> Vec<u8> {
        let comp = literal_stream(data);
        let key = 0x0123_4567_89AB_CDEFu64;
        let mut seq = key.to_le_bytes().to_vec();
        seq.extend(key.rotate_left((key & 0x1F) as u32).to_le_bytes());
        let mut d = Vec::new();
        put(&mut d, crc64(crc64_seed1(0, 16), &seq));
        put(&mut d, key);
        put(&mut d, crc64(crc64_seed2(0, comp.len()), &comp));
        d.extend((comp.len() as u32).to_le_bytes());
        d.extend(0u32.to_le_bytes());
        d.extend(comp);
        assert!(d.len() <= HEADER_BLOCKS * SYSTEM_K);
        d.resize(HEADER_BLOCKS * SYSTEM_K, 0);
        let mut page = encode_interleaved(&d, SYSTEM_K);
        page.resize(0x400, 0);
        page
    }

    fn serialize(m: &Model) -> Vec<u8> {
        let mut file = vec![0u8; 0x80];
        file[..6].copy_from_slice(b"AC1021");
        file[OFF_MAINTENANCE] = 0x32;
        file[OFF_CODEPAGE] = 30;
        file.resize(0x480, 0); // header page, filled in last
        let mut entries: Vec<(u64, i64)> = Vec::new();
        for s in &m.secs {
            for p in &s.pages {
                file.extend(&p.file);
                entries.push((p.file.len() as u64, p.map_id));
            }
        }
        let next = entries.iter().map(|e| e.1).max().unwrap_or(0) + 1;
        let (sm_file, sm) = system_page(&section_map(m), m.repeat);
        let sm_id = next;
        file.extend(&sm_file);
        entries.push((sm_file.len() as u64, sm_id));
        entries.extend(&m.extra_entries);
        // The pages map lists itself: its size is known before its data is encoded.
        let pm_id = next + 1;
        let pm_len = (entries.len() + 1) * 16;
        let pm_size = (pm_len.div_ceil(8) * 8 * m.repeat as usize).div_ceil(SYSTEM_K) * CODEWORD;
        entries.push((pm_size.div_ceil(0x20) as u64 * 0x20, pm_id));
        let mut pm_data = Vec::new();
        for &(size, id) in &entries {
            put(&mut pm_data, size);
            put(&mut pm_data, id as u64);
        }
        let pm_at = file.len() as u64;
        let (pm_file, pm) = system_page(&pm_data, m.repeat);
        assert_eq!(pm_file.len() as u64, entries.last().unwrap().0);
        file.extend(&pm_file);
        file.resize(file.len() + m.tail_padding, 0);

        let copy_at = file.len() as u64;
        let mut h = vec![0u8; HEADER_DATA_LEN];
        let set =
            |h: &mut Vec<u8>, at: usize, v: u64| h[at..at + 8].copy_from_slice(&v.to_le_bytes());
        set(&mut h, 0x00, 0x70);
        set(&mut h, 0x08, copy_at + 0x400);
        set(&mut h, 0x10, pm.crc_compressed);
        set(&mut h, 0x18, pm.repeat);
        set(&mut h, 0x38, pm_at - PAGE_BASE);
        set(&mut h, 0x40, pm_id as u64);
        set(&mut h, 0x48, copy_at - PAGE_BASE);
        set(&mut h, 0x50, pm.compressed);
        set(&mut h, 0x58, pm.uncompressed);
        set(&mut h, 0x80, pm.crc_uncompressed);
        set(&mut h, 0xA8, sm.crc_uncompressed);
        set(&mut h, 0xB0, sm.compressed);
        set(&mut h, 0xC0, sm_id as u64);
        set(&mut h, 0xC8, sm.uncompressed);
        set(&mut h, 0xD0, sm.crc_compressed);
        set(&mut h, 0xD8, sm.repeat);
        for &(at, v) in &m.header_fields {
            set(&mut h, at, v);
        }
        let crc = crc64(crc64_seed2(0, HEADER_DATA_LEN), &h);
        set(&mut h, HEADER_CRC_AT, crc);
        let page = header_page(&h);
        file[0x80..0x480].copy_from_slice(&page);
        file.extend(&page);
        file
    }

    fn build() -> Vec<u8> {
        serialize(&model())
    }

    #[test]
    fn repeat_stream_decodes() {
        let s = repeat_stream(b"ABCDEFGH", 0x2000);
        assert_eq!(
            lz77_2007::decompress(&s, 0x2000).unwrap(),
            b"ABCDEFGH".repeat(0x400)
        );
    }

    #[test]
    fn synthetic_valid_file_reads() {
        let raw = read(&build()).unwrap();
        assert!(raw.warnings.is_empty(), "{:?}", raw.warnings);
        for s in sections() {
            assert_eq!(
                raw.sections[&SectionName::from_r2004_name(s.name)],
                s.data,
                "{:?}",
                s.name
            );
        }
        assert_eq!(raw.sections.len(), sections().len());
        assert_eq!(raw.version, Version::R2007);
        assert_eq!((raw.maintenance, raw.codepage), (0x32, 30));
        assert_eq!(raw.objects_base, 0);
        assert_eq!(raw.preview_offset, None);
    }

    fn read_model(m: &Model) -> Result<RawSections> {
        read(&serialize(m))
    }

    fn assert_corrupt(r: Result<RawSections>, what: &str) {
        assert!(matches!(r, Err(Error::Corrupt(_))), "{what}: {r:?}");
    }

    /// File address and RS block count of the pages map.
    fn pages_map_location(file: &[u8]) -> (usize, usize) {
        let h = decode_file_header(file, FILE_HEADER_AT).unwrap();
        let at = (PAGE_BASE + h.pages_map_offset) as usize;
        let aligned = h.pages_map.compressed.div_ceil(8) * 8;
        (
            at,
            (aligned * h.pages_map.repeat).div_ceil(SYSTEM_K as u64) as usize,
        )
    }

    fn pages(file: &[u8]) -> BTreeMap<u64, Page> {
        let h = decode_file_header(file, FILE_HEADER_AT).unwrap();
        let (at, _) = pages_map_location(file);
        parse_pages_map(
            &read_system_page(file, at as u64, None, &h.pages_map, 0, "pages map").unwrap(),
        )
        .unwrap()
    }

    /// A prefix that cuts into the data fails; one that holds every section (lacking only
    /// the trailing header copy) reads them all, with a warning that it was cut.
    #[test]
    fn every_prefix_errors_or_warns() {
        let file = build();
        let whole = read(&file).unwrap();
        for n in 0..file.len() {
            if let Ok(raw) = read(&file[..n]) {
                assert_eq!(raw.sections, whole.sections, "prefix {n:#x}");
                assert!(
                    raw.warnings
                        .iter()
                        .any(|w| w.contains("every section was read")),
                    "prefix {n:#x}: {:?}",
                    raw.warnings
                );
            }
        }
    }

    #[test]
    fn every_byte_flip_is_handled() {
        let file = build();
        for i in 0..file.len() {
            let mut bad = file.clone();
            bad[i] ^= 0xFF;
            let _ = read(&bad); // must not panic
        }
    }

    #[test]
    fn correctable_errors_are_fixed() {
        let file = build();
        let good = read(&file).unwrap();
        let (pm_at, pm_blocks) = pages_map_location(&file);
        // The Header's only page is the first page (one RS(255,251) block) at 0x480.
        let header_page = pages(&file)[&model().secs[0].pages[0].r.id].address as usize;
        assert_eq!(header_page, 0x480);
        let cases: Vec<Vec<usize>> = vec![
            vec![header_page, header_page + 254],
            (0..8).map(|j| pm_at + j * pm_blocks).collect(),
            (0..8).map(|j| 0x80 + j * 3 + 1).collect(),
            // The header copy at the end.
            (0..8).map(|j| file.len() - 0x400 + j * 3).collect(),
        ];
        for c in cases {
            let mut bad = file.clone();
            for &p in &c {
                bad[p] ^= 0xA5;
            }
            let raw = read(&bad).unwrap_or_else(|e| panic!("{c:x?}: {e}"));
            assert_eq!(raw.sections, good.sections, "{c:x?}");
            assert!(raw.warnings.is_empty(), "{c:x?}: {:?}", raw.warnings);
        }
    }

    #[test]
    fn three_errors_in_a_data_codeword_are_never_silent() {
        // AcDb:Unheard is optional and stored in one RS(255,251) block: three byte errors exceed
        // the code. RS either reports the codeword (section skipped) or miscorrects it, and then
        // the page CRC fails. Either way there is a warning.
        let m = model();
        let file = serialize(&m);
        let at = pages(&file)[&m.secs[5].pages[0].r.id].address as usize;
        for seed in 0..40 {
            let mut bad = file.clone();
            for e in 0..3 {
                bad[at + (seed * 37 + e * 71) % 255] ^= (seed as u8).wrapping_mul(13) | 1;
            }
            let raw = read(&bad).unwrap();
            assert!(!raw.warnings.is_empty(), "seed {seed}");
            let unheard = raw.sections.get(&SectionName::Other("AcDb:Unheard".into()));
            assert!(unheard.is_none() || raw.warnings.iter().any(|w| w.contains("bad CRC")));
        }
    }

    #[test]
    fn data_page_crc_and_checksum_mismatches_are_warnings() {
        for field in 0..2 {
            let mut m = model();
            let r = &mut m.secs[0].pages[0].r;
            if field == 0 {
                r.crc ^= 1;
            } else {
                r.checksum ^= 1;
            }
            let raw = read_model(&m).unwrap();
            assert_eq!(raw.warnings.len(), 1, "{:?}", raw.warnings);
            assert_eq!(raw.sections, read(&build()).unwrap().sections);
        }
    }

    #[test]
    fn system_page_crc_mismatch_is_bad_crc() {
        // Pages map compressed / uncompressed CRC, section map compressed / uncompressed CRC.
        for at in [0x10, 0x80, 0xD0, 0xA8] {
            let mut m = model();
            m.header_fields.push((at, 0x1234));
            assert!(
                matches!(read_model(&m), Err(Error::BadCrc { .. })),
                "{at:#x}"
            );
        }
    }

    #[test]
    fn file_header_crcs_are_checked() {
        let file = build();
        let good = decode_file_header(&file, FILE_HEADER_AT).unwrap();
        let mut h = good.data.clone();
        h[0x70] ^= 1; // covered by the header data CRC only
        let page = header_page(&h);
        assert!(matches!(
            decode_file_header(&page, 0),
            Err(Error::BadCrc { context }) if context.contains("data")
        ));
        // Undecodable: 9 errors in one RS(255,239) codeword.
        let mut bad = file.clone();
        for j in 0..9 {
            bad[0x80 + j * 3] ^= 0x77;
        }
        assert!(read(&bad).is_err());
    }

    #[test]
    fn header_copy_and_file_size() {
        let file = build();
        let len = file.len() as u64;
        let mut m = model();
        m.header_fields.push((0x08, len + 1));
        let raw = read_model(&m).unwrap();
        assert!(raw.warnings[0].contains("cut short"), "{:?}", raw.warnings);
        let mut m = model();
        m.header_fields.push((0x08, len - 1));
        let raw = read_model(&m).unwrap();
        assert_eq!(raw.warnings.len(), 1, "{:?}", raw.warnings);

        // A copy whose data differs is a warning; so is a missing copy when every section
        // reads.
        let mut h = decode_file_header(&file, FILE_HEADER_AT).unwrap().data;
        h[0x70] ^= 1;
        h[HEADER_CRC_AT..].fill(0);
        let crc = crc64(crc64_seed2(0, HEADER_DATA_LEN), &h);
        h[HEADER_CRC_AT..].copy_from_slice(&crc.to_le_bytes());
        let mut bad = file.clone();
        let copy_at = file.len() - 0x400;
        bad[copy_at..].copy_from_slice(&header_page(&h));
        let raw = read(&bad).unwrap();
        assert!(raw.warnings[0].contains("differs"), "{:?}", raw.warnings);
        let mut m = model();
        m.header_fields.push((0x48, u64::MAX));
        let raw = read_model(&m).unwrap();
        assert!(
            raw.warnings
                .iter()
                .any(|w| w.contains("repeated R2007 file header missing")),
            "{:?}",
            raw.warnings
        );
    }

    #[test]
    fn page_shared_by_two_sections_errors() {
        let mut m = model();
        m.secs[2].pages[0].r = m.secs[1].pages[0].r.clone();
        m.secs[2].size = m.secs[1].size;
        assert!(matches!(
            read_model(&m),
            Err(Error::Corrupt(msg)) if msg.contains("already used")
        ));
    }

    #[test]
    fn system_pages_cannot_be_section_pages() {
        let file = build();
        let h = decode_file_header(&file, FILE_HEADER_AT).unwrap();
        for id in [h.pages_map_id, h.sections_map_id] {
            let mut m = model();
            m.secs[2].pages[0].r.id = id; // AcDb:Handles
            assert!(
                matches!(read_model(&m), Err(Error::Corrupt(e)) if e.contains("already used")),
                "{id}"
            );
        }
    }

    #[test]
    fn long_section_names_are_rejected() {
        let mut m = model();
        m.secs[4].name = "N".repeat(MAX_NAME_LEN as usize / 2 - 1); // 0x100 bytes with NUL
        let raw = read_model(&m).unwrap();
        assert!(
            raw.sections
                .contains_key(&SectionName::Other(m.secs[4].name.clone()))
        );
        m.secs[4].name.push('N');
        assert!(matches!(
            read_model(&m),
            Err(Error::Corrupt(e)) if e.contains("name length")
        ));
    }

    #[test]
    fn warnings_stay_linear_with_many_bad_pages() {
        // An optional encoding-1 section of many small pages, each with a bad CRC and checksum,
        // under the longest accepted name: warnings are bounded per page.
        let mut secs = sections();
        secs.push(Sec {
            name: "AcDb:Many",
            data: vec![7; 0x20 * 200],
            encoding: 1,
            compressed: false,
            page_size: 0x20,
        });
        let mut m = model_with(&secs);
        let last = m.secs.len() - 1;
        m.secs[last].name = "\u{1}".repeat(MAX_NAME_LEN as usize / 2 - 1);
        for p in &mut m.secs[last].pages {
            p.r.crc ^= 1;
            p.r.checksum ^= 1;
        }
        let file = serialize(&m);
        let raw = read(&file).unwrap();
        assert_eq!(raw.warnings.len(), 400);
        let total: usize = raw.warnings.iter().map(String::len).sum();
        assert!(total < 400 * 0x400, "{total:#x}");
    }

    #[test]
    fn encrypted_required_section_is_encrypted_error() {
        let mut m = model_with(&sections());
        let objects = m
            .secs
            .iter_mut()
            .find(|s| s.name == "AcDb:AcDbObjects")
            .unwrap();
        objects.encrypted = 1;
        assert!(matches!(
            read_model(&m),
            Err(Error::Encrypted(name)) if name == "AcDb:AcDbObjects"
        ));
    }

    #[test]
    fn duplicate_section_errors() {
        let mut secs = sections();
        secs.push(secs[0].clone());
        assert!(matches!(
            read_model(&model_with(&secs)),
            Err(Error::Corrupt(msg)) if msg.contains("duplicate section")
        ));
    }

    #[test]
    fn missing_required_section_errors() {
        let secs: Vec<Sec> = sections()
            .into_iter()
            .filter(|s| s.name != "AcDb:Handles")
            .collect();
        assert!(matches!(
            read_model(&model_with(&secs)),
            Err(Error::Corrupt(msg)) if msg.contains("AcDb:Handles")
        ));
    }

    #[test]
    fn optional_section_failures_are_warnings() {
        let edits: [fn(&mut SecModel); 4] = [
            |s| s.encoding = 7,
            |s| s.encrypted = 1,
            |s| s.pages[0].r.id = 999,
            // Larger than its 0x20-byte page.
            |s| {
                s.size = 0x400;
                s.pages[0].r.compressed = 0x400;
                s.pages[0].r.uncompressed = 0x400;
            },
        ];
        for (i, edit) in edits.iter().enumerate() {
            let mut m = model();
            edit(&mut m.secs[4]); // AcDb:AppInfo
            let raw = read_model(&m).unwrap();
            assert!(!raw.sections.contains_key(&SectionName::AppInfo), "{i}");
            assert_eq!(raw.warnings.len(), 1, "{i}: {:?}", raw.warnings);
            assert!(raw.warnings[0].contains("AcDb:AppInfo"), "{i}");
        }
    }

    #[test]
    fn absurd_page_fields_error() {
        type Edit = (&'static str, fn(&mut PageRef));
        let edits: [Edit; 8] = [
            ("u max", |r| r.uncompressed = u64::MAX),
            ("c max", |r| r.compressed = u64::MAX),
            ("c zero", |r| r.compressed = 0),
            ("c > u", |r| r.compressed = r.uncompressed + 1),
            ("offset max", |r| r.offset = u64::MAX),
            ("offset past end", |r| r.offset = 0x10_0000),
            ("not in map", |r| r.id = 12345),
            // Stored, but more RS blocks than the page holds.
            ("blocks", |r| r.compressed = r.uncompressed),
        ];
        for (what, edit) in edits {
            let mut m = model();
            edit(&mut m.secs[0].pages[0].r);
            assert_corrupt(read_model(&m), what);
        }
        // Each guard by its own message, where another check would also reject the file.
        type Case = (&'static str, usize, fn(&mut PageRef));
        let cases: [Case; 2] = [
            // AcDb:Handles: 2 bytes stored in one block; 3 still fit that block.
            ("out of range", 2, |r| r.compressed = r.uncompressed + 1),
            ("do not fit", 0, |r| r.compressed = r.uncompressed),
        ];
        for (msg, sec, edit) in cases {
            let mut m = model();
            edit(&mut m.secs[sec].pages[0].r);
            let r = read_model(&m);
            assert!(
                matches!(&r, Err(Error::Corrupt(e)) if e.contains(msg)),
                "{msg}: {r:?}"
            );
        }
        // Pages out of order: the second Objects page placed before the first one's end.
        let mut m = model();
        m.secs[3].pages[1].r.offset = 0x100;
        assert_corrupt(read_model(&m), "order");
        // A compressed page decompressing to more than PAGE_OUTPUT_MAX.
        let mut m = model();
        m.secs[0].size = PAGE_OUTPUT_MAX + 1;
        m.secs[0].pages[0].r.uncompressed = PAGE_OUTPUT_MAX + 1;
        assert!(matches!(
            read_model(&m),
            Err(Error::Corrupt(msg)) if msg.contains("out of range")
        ));
        // Section sizes beyond the budget.
        for size in [u64::MAX, 1 << 40] {
            let mut m = model();
            m.secs[0].size = size;
            assert_corrupt(read_model(&m), "size");
        }
    }

    #[test]
    fn page_past_eof_is_truncated() {
        let mut m = model();
        // Page 499 runs far past the end of the file; page 500 starts after it.
        m.extra_entries.extend([(0x1000_0000, 499), (0x100, 500)]);
        m.secs[0].pages[0].r.id = 500;
        assert!(matches!(read_model(&m), Err(Error::Truncated { .. })));
    }

    #[test]
    fn bad_pages_map_entries_error() {
        for size in [0, 0x10, 0x21] {
            let mut m = model();
            m.extra_entries.push((size, 400));
            assert_corrupt(read_model(&m), &format!("{size:#x}"));
        }
        let mut m = model();
        m.extra_entries.push((0x20, -1)); // |id| = 1 duplicates the first data page
        assert!(matches!(
            read_model(&m),
            Err(Error::Corrupt(e)) if e.contains("duplicate page")
        ));
    }

    #[test]
    fn bad_system_page_fields_error() {
        // (field, value): pages map repeat, sizes; section map id, repeat.
        for (at, v) in [
            (0x18, 0),
            (0x18, u64::MAX),
            (0x50, 0),
            (0x58, u64::MAX),
            (0x50, 0x100), // compressed > uncompressed
            (0xC0, 999),
            (0xD8, 50), // more blocks than the section map page holds
        ] {
            let mut m = model();
            m.header_fields.push((at, v));
            assert_corrupt(read_model(&m), &format!("{at:#x}={v:#x}"));
        }
        let mut m = model();
        m.header_fields.push((0x18, 1000)); // blocks past the end of the file
        assert!(matches!(read_model(&m), Err(Error::Truncated { .. })));
    }

    #[test]
    fn zero_sections_share_one_budget() {
        let base = build();
        let budget: u64 = pages(&base)
            .values()
            .map(|p| {
                if p.size >= MIN_ENCODED_PAGE {
                    p.size.max(PAGE_OUTPUT_MAX)
                } else {
                    p.size
                }
            })
            .sum::<u64>();
        let mut secs = sections();
        for i in 0..10 {
            secs.push(Sec {
                name: Box::leak(format!("AcDb:Zero{i}").into_boxed_str()),
                data: vec![0; budget as usize / 4],
                encoding: 4,
                compressed: false,
                page_size: 0xF800,
            });
        }
        let file = serialize(&model_with(&secs));
        let raw = read(&file).unwrap();
        assert!(
            raw.warnings.iter().any(|w| w.contains("budget")),
            "{:?}",
            raw.warnings
        );
        let total: u64 = raw.sections.values().map(|s| s.len() as u64).sum();
        assert!(total <= budget + 2 * file.len() as u64, "{total:#x}");
        assert!(
            raw.sections
                .contains_key(&SectionName::Other("AcDb:Zero0".into()))
        );
    }
}
