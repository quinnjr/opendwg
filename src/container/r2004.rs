//! R2004-family file reader (AC1018, AC1024, AC1027, AC1032), ODS chapter 4.
//!
//! ODS chapters 6-8 say the R2010, R2013 and R2018 file header, page map, section map and
//! compression are the same as R2004, so this module does not branch on version. The only
//! differences at this level are which section names appear (R2013+ adds
//! `AcDb:AcDsPrototype_1b`), which [`SectionName::from_r2004_name`] already handles, and the
//! per-section page sizes, which are read from the section map rather than assumed.
//!
//! Stages:
//! 1. [`decrypt_file_header`]: the 0x6C encrypted bytes at 0x80 (ODS 4.1), ID string and CRC-32.
//! 2. [`read_system_page`]: the section page map, then the section map (ODS 4.3-4.5).
//! 3. [`read_section`]: each data page header is unmasked (ODS 4.6), checksummed, decompressed
//!    (ODS 4.7) and the logical page prefixes concatenated, with omitted zero pages restored.
//!
//! Policies:
//! - A system page checksum mismatch is `Err(Error::BadCrc)`: nothing can be located without the
//!   maps. A data page checksum mismatch is a warning in [`RawSections::warnings`] and the page is
//!   still used (ODS notes AutoCAD tolerates some bad checksums).
//! - Header, Classes, Handles and Objects are required: any failure reading them is an error.
//!   Other sections that fail to read are skipped with a warning.
//! - Unknown section names are kept as [`SectionName::Other`]; the empty-named first section
//!   (ODS chapter 12) is kept as `Other("")`.
//! - `objects_base` is 0: object map offsets are relative to the `AcDb:AcDbObjects` section.
//! - `preview_offset` is the file header's preview address (ODS 4.1: the image page plus its
//!   0x20-byte header), when nonzero and inside the file.
//!
//! Allocation and work bounds (every size from the file is checked before it is used):
//! - A system page's decompressed size may not exceed the file size. Each page-map entry
//!   (8 or 24 bytes) and section-map page reference (16 bytes) describes a page of at least 0x20
//!   bytes, so a genuine map is far smaller.
//! - Page-map entries must be a nonzero multiple of 0x20 bytes (ODS 4.3: pages start on 0x20
//!   boundaries). Entries running past the end of the file are kept, since the page map's own
//!   entry does so in every corpus file, but they do not count as pages in the file.
//! - A page may be referenced by only one section (ODS 4.4: page numbers are unique per file),
//!   and section ids and names must be unique, so every page is checksummed and decompressed at
//!   most once.
//! - A compressed section's page size is at most 0x7400 (ODS 4.5 table); an uncompressed
//!   section stores whole pages, so its page size is at most the file size.
//! - A section may span at most as many pages as lie inside the file, and all sections together
//!   at most 0x7400 bytes per in-file page plus the file size (for an oversized uncompressed
//!   page and small all-zero sections). In-file pages number at most `len / 0x20`, so the total
//!   output is at most about 928 times the input size.

use std::collections::{BTreeMap, BTreeSet};

use super::{
    R2004_PLAIN_HEADER_LEN, RawSections, SectionMeta, SectionName, check_required, is_required,
    span, u32_at, u64_at,
};
use crate::codec::{crc32, lz77_2004, page_checksum, page_header_mask, xor_r2004_file_header};
use crate::error::{Error, Result};
use crate::version::Version;

pub(super) const FILE_ID: [u8; 12] = *b"AcFssFcAJMB\0";
pub(super) const OFF_MAINTENANCE: usize = 0x0B;
pub(super) const OFF_PREVIEW: usize = 0x0D;
pub(super) const OFF_CODEPAGE: usize = 0x13;
pub(super) const ENCRYPTED_AT: usize = 0x80;
pub(super) const ENCRYPTED_LEN: usize = 0x6C;
/// File address of the first section page (ODS 4.4).
pub(super) const PAGE_BASE: u64 = 0x100;

/// System page type: section page map (ODS 4.3).
pub(super) const PAGE_MAP_KIND: u32 = 0x4163_0E3B;
/// System page type: section map (ODS 4.3).
pub(super) const SECTION_MAP_KIND: u32 = 0x4163_003B;
/// Data page type (ODS 4.6).
pub(super) const DATA_PAGE_KIND: u32 = 0x4163_043B;
pub(super) const SYSTEM_HEADER_LEN: usize = 0x14;
pub(super) const DATA_HEADER_LEN: usize = 0x20;
/// Page size of every compressed section (ODS 4.5 table).
pub(super) const COMPRESSED_PAGE_MAX: u32 = 0x7400;
pub(super) const SECTION_MAP_HEADER_LEN: usize = 0x14;
pub(super) const SECTION_DESC_LEN: usize = 0x60;
pub(super) const PAGE_REF_LEN: usize = 0x10;

/// The fields of the decrypted R2004 file header data that the reader uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct R2004FileHeader {
    /// Absolute file address of the section page map (the stored value plus 0x100).
    pub page_map_address: u64,
    /// Page number of the section map in the section page map.
    pub section_map_id: u32,
    /// File address of the repeated header data at the end of the file (stored as is, with
    /// no 0x100 added; checked against the corpus).
    pub second_header_address: u64,
    /// The decrypted 0x6C bytes, for comparison with the repeated copy.
    pub data: [u8; ENCRYPTED_LEN],
}

/// A page of the file, located through the section page map.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Page {
    pub address: u64,
    pub size: u64,
}

/// A reference to one stored page of a data section (ODS 4.5, local section page map).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PageRef {
    pub number: u32,
    pub data_size: u32,
    pub start: u64,
}

/// One section description from the section map (ODS 4.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SectionDesc {
    pub name: String,
    pub size: u64,
    pub max_page_size: u32,
    /// The field at 0x10 (ODS: unknown).
    pub unknown: u32,
    /// 1 = stored, 2 = compressed.
    pub compressed: u32,
    pub section_id: u32,
    /// 0 = no, 1 = password-encrypted, 2 = unknown (ODS 4.5).
    pub encrypted: u32,
    pub pages: Vec<PageRef>,
}

/// Decrypt and check the 0x6C-byte file header data at 0x80 of `bytes` (ODS 4.1).
///
/// The ID string must be `"AcFssFcAJMB\0"` and the CRC-32 (seed 0, computed over all 0x6C bytes
/// with the CRC field zeroed) must match.
pub(super) fn decrypt_file_header(bytes: &[u8]) -> Result<R2004FileHeader> {
    let mut data: [u8; ENCRYPTED_LEN] = bytes
        .get(ENCRYPTED_AT..ENCRYPTED_AT + ENCRYPTED_LEN)
        .and_then(|s| s.try_into().ok())
        .ok_or(Error::Truncated {
            context: "R2004 file header",
        })?;
    xor_r2004_file_header(&mut data);
    if data[..12] != FILE_ID {
        return Err(Error::BadSentinel("R2004 file header ID"));
    }
    let stored = u32_at(&data, 0x68, "R2004 file header")?;
    let mut zeroed = data;
    zeroed[0x68..0x6C].fill(0);
    if crc32(0, &zeroed) != stored {
        return Err(Error::BadCrc {
            context: "R2004 file header",
        });
    }
    let page_map_address = u64_at(&data, 0x54, "R2004 file header")?
        .checked_add(PAGE_BASE)
        .ok_or_else(|| Error::Corrupt("section page map address overflows".into()))?;
    Ok(R2004FileHeader {
        page_map_address,
        section_map_id: u32_at(&data, 0x5C, "R2004 file header")?,
        second_header_address: u64_at(&data, 0x34, "R2004 file header")?,
        data,
    })
}

/// Read, verify and decompress the system page of type `kind` at file address `offset`
/// (ODS 4.3). `kind` is `0x41630E3B` (section page map) or `0x4163003B` (section map).
///
/// The checksum is computed in two stages: over the 0x14-byte header (checksum field zeroed)
/// with seed 0, then over the compressed data seeded with that result.
pub(super) fn read_system_page(bytes: &[u8], offset: u64, kind: u32) -> Result<Vec<u8>> {
    let what = if kind == PAGE_MAP_KIND {
        "section page map"
    } else {
        "section map"
    };
    let header =
        span(bytes, offset, SYSTEM_HEADER_LEN as u64).ok_or(Error::Truncated { context: what })?;
    let page_kind = u32_at(header, 0x00, what)?;
    if page_kind != kind {
        return Err(Error::Corrupt(format!(
            "{what} at {offset:#x}: page type {page_kind:#x}, expected {kind:#x}"
        )));
    }
    let decompressed = u32_at(header, 0x04, what)?;
    let compressed = u32_at(header, 0x08, what)?;
    let compression = u32_at(header, 0x0C, what)?;
    let stored = u32_at(header, 0x10, what)?;
    if compression != 2 {
        return Err(Error::Corrupt(format!(
            "{what}: compression type {compression}, expected 2"
        )));
    }
    let decompressed = usize::try_from(decompressed)
        .ok()
        .filter(|&d| d <= bytes.len())
        .ok_or_else(|| {
            Error::Corrupt(format!(
                "{what}: decompressed size {decompressed:#x} exceeds the file size"
            ))
        })?;
    let data = span(
        bytes,
        offset.saturating_add(SYSTEM_HEADER_LEN as u64),
        u64::from(compressed),
    )
    .ok_or(Error::Truncated { context: what })?;
    let mut h = [0u8; SYSTEM_HEADER_LEN];
    h.copy_from_slice(header);
    h[0x10..].fill(0);
    if page_checksum(page_checksum(0, &h), data) != stored {
        return Err(Error::BadCrc { context: what });
    }
    lz77_2004::decompress(data, decompressed)
}

/// Parse the decompressed section page map (ODS 4.4) into page number -> page location.
///
/// Addresses start at 0x100 and accumulate by each entry's size, gaps included. Gap entries
/// (negative numbers) carry 16 extra bytes and are not returned. Page sizes must be a nonzero
/// multiple of 0x20 (ODS 4.3).
pub(super) fn parse_page_map(data: &[u8]) -> Result<BTreeMap<u32, Page>> {
    const CTX: &str = "section page map";
    let mut pages = BTreeMap::new();
    let mut address = PAGE_BASE;
    let mut pos = 0;
    while pos < data.len() {
        let number = u32_at(data, pos, CTX)? as i32;
        let size = u64::from(u32_at(data, pos + 4, CTX)?);
        pos += 8;
        if number < 0 {
            if data.len() - pos < 16 {
                return Err(Error::Truncated { context: CTX });
            }
            pos += 16;
        } else if size == 0 || size % 0x20 != 0 {
            return Err(Error::Corrupt(format!(
                "section page map: page {number} has size {size:#x}"
            )));
        } else if pages
            .insert(number as u32, Page { address, size })
            .is_some()
        {
            return Err(Error::Corrupt(format!(
                "section page map: duplicate page {number}"
            )));
        }
        address = address
            .checked_add(size)
            .ok_or_else(|| Error::Corrupt("section page map: address overflows".into()))?;
    }
    Ok(pages)
}

/// Parse the decompressed section map (ODS 4.5). Counts are checked against the remaining
/// bytes before anything is allocated.
pub(super) fn parse_section_map(data: &[u8]) -> Result<Vec<SectionDesc>> {
    const CTX: &str = "section map";
    let count = u32_at(data, 0, CTX)?;
    let mut pos = SECTION_MAP_HEADER_LEN;
    let mut descs = Vec::new();
    for _ in 0..count {
        let d = data
            .get(pos..pos.saturating_add(SECTION_DESC_LEN))
            .ok_or(Error::Truncated { context: CTX })?;
        pos += SECTION_DESC_LEN;
        let page_count = u32_at(d, 0x08, CTX)?;
        let refs_len = usize::try_from(page_count)
            .ok()
            .and_then(|n| n.checked_mul(PAGE_REF_LEN))
            .filter(|&n| n <= data.len() - pos)
            .ok_or_else(|| {
                Error::Corrupt(format!(
                    "section map: page count {page_count} exceeds the map"
                ))
            })?;
        let name = &d[0x20..0x60];
        let name = &name[..name.iter().position(|&b| b == 0).unwrap_or(name.len())];
        let pages = data[pos..pos + refs_len]
            .chunks_exact(PAGE_REF_LEN)
            .map(|r| {
                Ok(PageRef {
                    number: u32_at(r, 0, CTX)?,
                    data_size: u32_at(r, 4, CTX)?,
                    start: u64_at(r, 8, CTX)?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        pos += refs_len;
        descs.push(SectionDesc {
            name: String::from_utf8_lossy(name).into_owned(),
            size: u64_at(d, 0x00, CTX)?,
            max_page_size: u32_at(d, 0x0C, CTX)?,
            unknown: u32_at(d, 0x10, CTX)?,
            compressed: u32_at(d, 0x14, CTX)?,
            section_id: u32_at(d, 0x18, CTX)?,
            encrypted: u32_at(d, 0x1C, CTX)?,
            pages,
        });
    }
    Ok(descs)
}

/// State shared by the sections of one file while they are read.
struct SectionReader<'a> {
    bytes: &'a [u8],
    page_map: &'a BTreeMap<u32, Page>,
    /// Page-map entries lying wholly inside the file.
    in_file_pages: u64,
    /// Output still allowed for the whole file; each section's size is taken from it.
    budget: u64,
    /// Page numbers already referenced by a section.
    used_pages: BTreeSet<u32>,
    warnings: Vec<String>,
}

/// Read one data section: unmask each page header (ODS 4.6), verify its checksums, decompress
/// and append the logical prefix of each page, inserting omitted zero pages (ODS 4.5).
///
/// Checksum mismatches are pushed to `warnings`; everything else malformed is an error. Sizes
/// are validated before decompression: each page decompresses to exactly the section's page
/// size, which is at most 0x7400 for compressed sections. The section size must fit in the
/// pages inside the file and in the remaining budget. A page already used by another section
/// is an error.
fn read_section(st: &mut SectionReader<'_>, desc: &SectionDesc) -> Result<Vec<u8>> {
    let bytes = st.bytes;
    let name = &desc.name;
    let corrupt = |msg: String| Error::Corrupt(format!("section {name:?}: {msg}"));
    if desc.encrypted == 1 {
        return Err(Error::Encrypted(name.clone()));
    }
    let page_limit = match desc.compressed {
        2 => COMPRESSED_PAGE_MAX as usize,
        1 => bytes.len(),
        c => return Err(corrupt(format!("unknown compression flag {c}"))),
    };
    let page_size = usize::try_from(desc.max_page_size)
        .ok()
        .filter(|&p| p > 0 && p <= page_limit)
        .ok_or_else(|| corrupt(format!("page size {:#x} out of range", desc.max_page_size)))?;
    let size = usize::try_from(desc.size)
        .ok()
        .filter(|_| desc.size.div_ceil(page_size as u64) <= st.in_file_pages)
        .ok_or_else(|| {
            corrupt(format!(
                "size {:#x} needs more pages than the file holds",
                desc.size
            ))
        })?;
    if desc.size > st.budget {
        return Err(corrupt(format!(
            "size {:#x} exceeds the output budget left for this file ({:#x})",
            desc.size, st.budget
        )));
    }
    st.budget -= desc.size;

    let mut out = Vec::new();
    for r in &desc.pages {
        // Omitted zero pages precede a page whose start is past the data so far.
        let start = usize::try_from(r.start)
            .ok()
            .filter(|&s| s >= out.len() && s < size && (s - out.len()) % page_size == 0)
            .ok_or_else(|| corrupt(format!("page start offset {:#x} out of order", r.start)))?;
        out.resize(start, 0);
        let logical = page_size.min(size - start);

        let page = st
            .page_map
            .get(&r.number)
            .ok_or_else(|| corrupt(format!("page {} not in the page map", r.number)))?;
        if !st.used_pages.insert(r.number) {
            return Err(corrupt(format!(
                "page {} is already used by another section",
                r.number
            )));
        }
        let raw = span(bytes, page.address, DATA_HEADER_LEN as u64).ok_or(Error::Truncated {
            context: "data page header",
        })?;
        let mask = page_header_mask(page.address);
        let mut header = [0u8; DATA_HEADER_LEN];
        for (dst, src) in header.chunks_exact_mut(4).zip(raw.chunks_exact(4)) {
            let w = u32::from_le_bytes([src[0], src[1], src[2], src[3]]) ^ mask;
            dst.copy_from_slice(&w.to_le_bytes());
        }
        let field = |at: usize| {
            u32::from_le_bytes([header[at], header[at + 1], header[at + 2], header[at + 3]])
        };
        let (kind, section, data_size) = (field(0x00), field(0x04), field(0x08));
        let page_extent = u64::from(field(0x0C));
        let start_offset = u64::from(field(0x10)) | u64::from(field(0x14)) << 32;
        let (stored_header_sum, stored_data_sum) = (field(0x18), field(0x1C));
        if kind != DATA_PAGE_KIND {
            return Err(corrupt(format!(
                "page {} at {:#x}: type {kind:#x}, expected {DATA_PAGE_KIND:#x}",
                r.number, page.address
            )));
        }
        if section != desc.section_id {
            return Err(corrupt(format!(
                "page {} belongs to section {section}",
                r.number
            )));
        }
        if start_offset != r.start {
            return Err(corrupt(format!(
                "page {}: start offset {start_offset:#x}, section map says {:#x}",
                r.number, r.start
            )));
        }
        if data_size != r.data_size {
            return Err(corrupt(format!(
                "page {}: data size {data_size:#x}, section map says {:#x}",
                r.number, r.data_size
            )));
        }
        if page_extent != page.size {
            return Err(corrupt(format!(
                "page {}: header page size {page_extent:#x}, page map says {:#x}",
                r.number, page.size
            )));
        }
        if u64::from(data_size) + DATA_HEADER_LEN as u64 > page.size {
            return Err(corrupt(format!(
                "page {}: data size {data_size:#x} exceeds the page size {:#x}",
                r.number, page.size
            )));
        }
        let data = span(
            bytes,
            page.address.saturating_add(DATA_HEADER_LEN as u64),
            u64::from(data_size),
        )
        .ok_or(Error::Truncated {
            context: "data page",
        })?;

        // ODS 4.6: data checksum with seed 0, then the header (its own checksum field zeroed)
        // seeded with the data checksum.
        let data_sum = page_checksum(0, data);
        header[0x18..0x1C].fill(0);
        if data_sum != stored_data_sum || page_checksum(data_sum, &header) != stored_header_sum {
            st.warnings.push(format!(
                "section {name:?}: page {} at {:#x} has a bad checksum",
                r.number, page.address
            ));
        }

        if desc.compressed == 2 {
            let page_data = lz77_2004::decompress(data, page_size)?;
            out.extend_from_slice(&page_data[..logical]);
        } else {
            // ODS 4.5: a stored page holds the whole page image, padding included. One cut
            // short after the bytes the section uses loses only padding: read, with a warning.
            if data.len() < logical {
                return Err(corrupt(format!(
                    "page {}: stored size {data_size:#x} is shorter than the {logical:#x} bytes \
                     of data it holds",
                    r.number
                )));
            }
            if data.len() < page_size {
                st.warnings.push(format!(
                    "section {name:?}: page {}: stored size {data_size:#x} is shorter than the \
                     page size {page_size:#x} (padding only; read)",
                    r.number
                ));
            }
            out.extend_from_slice(&data[..logical]);
        }
    }
    // Trailing zero pages are not stored either.
    out.resize(size, 0);
    Ok(out)
}

pub(super) fn read(bytes: &[u8], version: Version) -> Result<RawSections> {
    match read_whole(bytes, version) {
        Err(Error::Corrupt(_) | Error::BadCrc { .. } | Error::BadSentinel(_))
            if cut_short(bytes) =>
        {
            Err(Error::Truncated {
                context: "R2004 file (cut short)",
            })
        }
        r => r,
    }
}

/// Whether the file lacks its trailing header copy: a failure to read it then comes from data
/// cut off.
fn cut_short(bytes: &[u8]) -> bool {
    decrypt_file_header(bytes)
        .is_ok_and(|h| span(bytes, h.second_header_address, ENCRYPTED_LEN as u64).is_none())
}

/// [`read`], whose errors a cut file turns into `Truncated`.
fn read_whole(bytes: &[u8], version: Version) -> Result<RawSections> {
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
    let file_header = decrypt_file_header(bytes)?;
    let mut warnings = Vec::new();
    // ODS 4.1: the header data is repeated at the end of the file. A missing copy is a
    // warning when every section reads (see `copy_missing` below); a differing copy is
    // tolerated.
    let second: Option<[u8; ENCRYPTED_LEN]> = span(
        bytes,
        file_header.second_header_address,
        ENCRYPTED_LEN as u64,
    )
    .and_then(|s| s.try_into().ok());
    let copy_missing = second.is_none();
    if let Some(mut second) = second {
        xor_r2004_file_header(&mut second);
        if second != file_header.data {
            warnings.push("repeated R2004 file header differs from the first".to_owned());
        }
    }

    let page_map = parse_page_map(&read_system_page(
        bytes,
        file_header.page_map_address,
        PAGE_MAP_KIND,
    )?)?;
    let section_map = page_map.get(&file_header.section_map_id).ok_or_else(|| {
        Error::Corrupt(format!(
            "section map page {} not in the page map",
            file_header.section_map_id
        ))
    })?;
    let descs = parse_section_map(&read_system_page(
        bytes,
        section_map.address,
        SECTION_MAP_KIND,
    )?)?;

    let in_file_pages = page_map
        .values()
        .filter(|p| p.address.saturating_add(p.size) <= bytes.len() as u64)
        .count() as u64;
    let mut st = SectionReader {
        bytes,
        page_map: &page_map,
        in_file_pages,
        budget: in_file_pages
            .saturating_mul(u64::from(COMPRESSED_PAGE_MAX))
            .saturating_add(bytes.len() as u64),
        used_pages: BTreeSet::new(),
        warnings,
    };
    let mut skipped = false;
    let mut names_seen = BTreeSet::new();
    let mut ids_seen = BTreeSet::new();
    let mut sections = BTreeMap::new();
    let mut section_meta = BTreeMap::new();
    for desc in &descs {
        let name = SectionName::from_r2004_name(&desc.name);
        let required = is_required(&name);
        if !names_seen.insert(name.clone()) {
            return Err(Error::Corrupt(format!(
                "section map: duplicate section {:?}",
                desc.name
            )));
        }
        if !ids_seen.insert(desc.section_id) {
            return Err(Error::Corrupt(format!(
                "section map: duplicate section id {}",
                desc.section_id
            )));
        }
        match read_section(&mut st, desc) {
            Ok(data) => {
                section_meta.insert(
                    name.clone(),
                    SectionMeta {
                        compressed: desc.compressed == 2,
                        encrypted: desc.encrypted,
                        max_page_size: desc.max_page_size,
                        unknown: desc.unknown,
                    },
                );
                sections.insert(name, data);
            }
            Err(e) if !required => {
                skipped = true;
                st.warnings
                    .push(format!("section {:?} skipped: {e}", desc.name));
            }
            Err(e) => return Err(e),
        }
    }
    let mut warnings = st.warnings;
    check_required(&sections)?;
    if copy_missing {
        // Without its trailing header copy and with a section unreadable, the file was cut
        // short: data is missing. With every section read, only the copy is.
        if skipped {
            return Err(Error::Truncated {
                context: "R2004 file (its repeated header and a section are missing)",
            });
        }
        warnings.push(
            "repeated R2004 file header missing at the end of the file; every section was read"
                .to_owned(),
        );
    }

    let preview_offset = Some(u64::from(preview)).filter(|&p| p != 0 && p < bytes.len() as u64);
    let file_header = bytes
        .get(..R2004_PLAIN_HEADER_LEN)
        .and_then(|h| h.try_into().ok());
    Ok(RawSections {
        version,
        maintenance,
        codepage,
        sections,
        objects_base: 0,
        preview_offset,
        section_meta,
        file_header,
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::lz77_2004::compress;

    struct Sec {
        name: &'static str,
        data: Vec<u8>,
        compressed: bool,
        page_size: u32,
    }

    /// Hostile edits applied while building a synthetic file. Section indexes refer to
    /// [`sections`].
    #[derive(Default)]
    struct Tweak {
        /// Stored section page map address (before the 0x100 is added).
        page_map_address: Option<u64>,
        page_count: Option<(usize, u32)>,
        size: Option<(usize, u64)>,
        max_page: Option<(usize, u32)>,
        /// Data size written for the first data page (its header and its section map entry).
        first_data_size: Option<u32>,
        /// Data size in the first page's section map entry only.
        first_ref_data_size: Option<u32>,
        /// Start offset in the first page's header only.
        first_header_start: Option<u64>,
        /// Page number of section 0's first page reference.
        first_ref_number: Option<u32>,
        /// Append a large gap and then page 500, which lies past the end of the file.
        far_page: bool,
        /// Extra page-map entries appended after everything else, as (number, size).
        extra_entries: Vec<(u32, u32)>,
        /// Page size written in the first data page's header only.
        first_header_page_size: Option<u32>,
        /// Section whose description says it is password-encrypted.
        encrypted: Option<usize>,
    }

    fn sections() -> Vec<Sec> {
        let mut objects: Vec<u8> = (0..0x100u32).map(|i| (i * 7) as u8).collect();
        objects.extend([0u8; 0x100]); // an all-zero page, omitted from the file
        objects.extend((0..0x80u32).map(|i| (i * 13 + 1) as u8));
        vec![
            Sec {
                name: "AcDb:Header",
                data: b"header variables".repeat(4),
                compressed: true,
                page_size: 0x7400,
            },
            Sec {
                name: "AcDb:Classes",
                data: b"classes".to_vec(),
                compressed: true,
                page_size: 0x7400,
            },
            Sec {
                name: "AcDb:Handles",
                data: vec![0, 0],
                compressed: true,
                page_size: 0x7400,
            },
            Sec {
                name: "AcDb:AcDbObjects",
                data: objects,
                compressed: true,
                page_size: 0x100,
            },
            Sec {
                name: "AcDb:AppInfo",
                data: b"app info".to_vec(),
                compressed: false,
                page_size: 0x80,
            },
            Sec {
                name: "AcDb:Unheard",
                data: vec![9; 3],
                compressed: true,
                page_size: 0x7400,
            },
        ]
    }

    fn pad32(b: &mut Vec<u8>) {
        b.resize(b.len().div_ceil(0x20) * 0x20, 0);
    }

    fn put32(b: &mut Vec<u8>, v: u32) {
        b.extend_from_slice(&v.to_le_bytes());
    }

    fn put64(b: &mut Vec<u8>, v: u64) {
        b.extend_from_slice(&v.to_le_bytes());
    }

    /// Append a system page of `kind` holding `data`; returns its address.
    fn system_page(file: &mut Vec<u8>, kind: u32, data: &[u8]) -> u64 {
        let at = file.len() as u64;
        let comp = compress(data);
        let mut h = Vec::new();
        for v in [kind, data.len() as u32, comp.len() as u32, 2, 0] {
            put32(&mut h, v);
        }
        let sum = page_checksum(page_checksum(0, &h), &comp);
        h[0x10..].copy_from_slice(&sum.to_le_bytes());
        file.extend(h);
        file.extend(comp);
        pad32(file);
        at
    }

    fn build_with(secs: &[Sec], t: &Tweak) -> Vec<u8> {
        let mut file = vec![0u8; 0x100];
        file[..6].copy_from_slice(b"AC1018");
        let mut page_map: Vec<u8> = Vec::new();
        let mut section_map = Vec::new();
        put32(&mut section_map, secs.len() as u32);
        for v in [2, 0x7400, 0, secs.len() as u32] {
            put32(&mut section_map, v);
        }
        let mut number = 1u32;
        for (i, s) in secs.iter().enumerate() {
            let page_size = s.page_size as usize;
            let mut refs = Vec::new();
            for (p, chunk) in s.data.chunks(page_size).enumerate() {
                if chunk.iter().all(|&b| b == 0) {
                    continue;
                }
                let mut page = chunk.to_vec();
                page.resize(page_size, 0);
                let payload = if s.compressed { compress(&page) } else { page };
                let address = file.len() as u64;
                let total = (0x20 + payload.len()).div_ceil(0x20) * 0x20;
                let start = (p * page_size) as u64;
                let data_size = match t.first_data_size {
                    Some(d) if number == 1 => d,
                    _ => payload.len() as u32,
                };
                let data_sum = page_checksum(0, &payload);
                let mut h = Vec::new();
                let header_total = t
                    .first_header_page_size
                    .filter(|_| number == 1)
                    .unwrap_or(total as u32);
                for v in [DATA_PAGE_KIND, i as u32 + 1, data_size, header_total] {
                    put32(&mut h, v);
                }
                put64(
                    &mut h,
                    t.first_header_start
                        .filter(|_| number == 1)
                        .unwrap_or(start),
                );
                put32(&mut h, 0);
                put32(&mut h, data_sum);
                let sum = page_checksum(data_sum, &h);
                h[0x18..0x1C].copy_from_slice(&sum.to_le_bytes());
                let mask = page_header_mask(address);
                for w in h.chunks_exact(4) {
                    put32(&mut file, u32::from_le_bytes(w.try_into().unwrap()) ^ mask);
                }
                file.extend(&payload);
                pad32(&mut file);
                put32(&mut page_map, number);
                put32(&mut page_map, total as u32);
                let ref_number = match t.first_ref_number {
                    Some(n) if i == 0 && refs.is_empty() => n,
                    _ => number,
                };
                put32(&mut refs, ref_number);
                put32(
                    &mut refs,
                    t.first_ref_data_size
                        .filter(|_| number == 1)
                        .unwrap_or(data_size),
                );
                put64(&mut refs, start);
                number += 1;
            }
            let pages = (refs.len() / PAGE_REF_LEN) as u32;
            let size = t
                .size
                .filter(|&(j, _)| j == i)
                .map_or(s.data.len() as u64, |v| v.1);
            put64(&mut section_map, size);
            put32(
                &mut section_map,
                t.page_count.filter(|&(j, _)| j == i).map_or(pages, |v| v.1),
            );
            put32(
                &mut section_map,
                t.max_page
                    .filter(|&(j, _)| j == i)
                    .map_or(s.page_size, |v| v.1),
            );
            put32(&mut section_map, 1);
            put32(&mut section_map, if s.compressed { 2 } else { 1 });
            put32(&mut section_map, i as u32 + 1);
            put32(&mut section_map, u32::from(t.encrypted == Some(i)));
            let mut name = s.name.as_bytes().to_vec();
            name.resize(64, 0);
            section_map.extend(name);
            section_map.extend(refs);
        }
        let section_map_at = system_page(&mut file, SECTION_MAP_KIND, &section_map);
        let section_map_id = number;
        put32(&mut page_map, section_map_id);
        put32(&mut page_map, (file.len() as u64 - section_map_at) as u32);
        if t.far_page {
            put32(&mut page_map, (-1i32) as u32);
            put32(&mut page_map, 0x1000_0000);
            page_map.extend([0u8; 16]);
            put32(&mut page_map, 500);
            put32(&mut page_map, 0x20);
        }
        for &(n, size) in &t.extra_entries {
            put32(&mut page_map, n);
            put32(&mut page_map, size);
        }
        let page_map_at = system_page(&mut file, PAGE_MAP_KIND, &page_map);

        let mut data = Vec::new();
        data.extend_from_slice(&FILE_ID);
        for v in [0, 0x6C, 4, 0, 0, 0, 1, number] {
            put32(&mut data, v);
        }
        put64(&mut data, 0);
        let second = file.len() as u64;
        put64(&mut data, second);
        for v in [0, number + 1, 0x20, 0x80, 0x40, number + 1] {
            put32(&mut data, v);
        }
        put64(
            &mut data,
            t.page_map_address.unwrap_or(page_map_at - PAGE_BASE),
        );
        for v in [section_map_id, 0, 0, 0] {
            put32(&mut data, v);
        }
        assert_eq!(data.len(), ENCRYPTED_LEN);
        let crc = crc32(0, &data);
        data[0x68..].copy_from_slice(&crc.to_le_bytes());
        xor_r2004_file_header(&mut data);
        file[ENCRYPTED_AT..ENCRYPTED_AT + ENCRYPTED_LEN].copy_from_slice(&data);
        file.extend(data);
        file
    }

    fn build(t: &Tweak) -> Vec<u8> {
        build_with(&sections(), t)
    }

    fn read_file(file: &[u8]) -> Result<RawSections> {
        read(file, Version::R2004)
    }

    #[test]
    fn synthetic_valid_file_reads() {
        let raw = read_file(&build(&Tweak::default())).unwrap();
        assert!(raw.warnings.is_empty(), "{:?}", raw.warnings);
        for s in sections() {
            assert_eq!(
                raw.sections[&SectionName::from_r2004_name(s.name)],
                s.data,
                "{}",
                s.name
            );
        }
        assert_eq!(raw.objects_base, 0);
        assert_eq!(raw.preview_offset, None);
    }

    #[test]
    fn decrypt_file_header_checks_id_and_crc() {
        let file = build(&Tweak::default());
        assert!(decrypt_file_header(&file).is_ok());
        assert!(matches!(
            decrypt_file_header(&file[..0x80 + 0x6B]),
            Err(Error::Truncated { .. })
        ));
        let mut bad = file.clone();
        bad[ENCRYPTED_AT] ^= 1; // ID string
        assert!(matches!(
            decrypt_file_header(&bad),
            Err(Error::BadSentinel(_))
        ));
        let mut bad = file.clone();
        bad[ENCRYPTED_AT + 0x5C] ^= 1; // section map id, covered by the CRC
        assert!(matches!(
            decrypt_file_header(&bad),
            Err(Error::BadCrc { .. })
        ));
    }

    #[test]
    fn bad_page_map_offset_errors() {
        for address in [u64::MAX, u64::MAX - 0x100, 0x1000_0000, 0] {
            let t = Tweak {
                page_map_address: Some(address),
                ..Tweak::default()
            };
            assert!(read_file(&build(&t)).is_err(), "{address:#x}");
        }
    }

    #[test]
    fn page_past_eof_errors() {
        let t = Tweak {
            first_ref_number: Some(500),
            far_page: true,
            ..Tweak::default()
        };
        assert!(matches!(
            read_file(&build(&t)),
            Err(Error::Truncated { .. })
        ));
        let t = Tweak {
            first_ref_number: Some(777),
            ..Tweak::default()
        };
        assert!(matches!(read_file(&build(&t)), Err(Error::Corrupt(_))));
    }

    #[test]
    fn huge_page_count_errors() {
        for n in [u32::MAX, 1_000_000] {
            let t = Tweak {
                page_count: Some((0, n)),
                ..Tweak::default()
            };
            assert!(
                matches!(read_file(&build(&t)), Err(Error::Corrupt(_))),
                "{n}"
            );
        }
    }

    #[test]
    fn data_size_beyond_page_errors() {
        let file = build(&Tweak::default());
        let header = decrypt_file_header(&file).unwrap();
        let pages = parse_page_map(
            &read_system_page(&file, header.page_map_address, PAGE_MAP_KIND).unwrap(),
        )
        .unwrap();
        let first = pages[&1].size as u32;
        // One byte more than the page holds after its 0x20-byte header, and far more.
        for size in [first - 0x1F, 0x10_0000, u32::MAX] {
            let t = Tweak {
                first_data_size: Some(size),
                ..Tweak::default()
            };
            assert!(
                matches!(read_file(&build(&t)), Err(Error::Corrupt(_))),
                "{size:#x}"
            );
        }
    }

    #[test]
    fn page_header_must_agree_with_section_map() {
        let t = Tweak {
            first_ref_data_size: Some(1),
            ..Tweak::default()
        };
        assert!(matches!(read_file(&build(&t)), Err(Error::Corrupt(m)) if m.contains("data size")));
        let t = Tweak {
            first_header_start: Some(0x7400),
            ..Tweak::default()
        };
        assert!(
            matches!(read_file(&build(&t)), Err(Error::Corrupt(m)) if m.contains("start offset"))
        );
    }

    #[test]
    fn absurd_section_sizes_error() {
        for size in [u64::MAX, 1 << 40, 0x7400 * 64] {
            let t = Tweak {
                size: Some((3, size)),
                ..Tweak::default()
            };
            assert!(
                matches!(read_file(&build(&t)), Err(Error::Corrupt(_))),
                "{size:#x}"
            );
        }
    }

    #[test]
    fn page_size_limits() {
        // Compressed sections: at most 0x7400; zero is never valid.
        for (sec, page) in [(0, 0x7401), (0, u32::MAX), (0, 0), (4, 0)] {
            let t = Tweak {
                max_page: Some((sec, page)),
                ..Tweak::default()
            };
            let r = read_file(&build(&t));
            if sec == 0 {
                assert!(matches!(r, Err(Error::Corrupt(_))), "{page:#x}");
            } else {
                // AppInfo is optional: skipped with a warning.
                let raw = r.unwrap();
                assert!(!raw.sections.contains_key(&SectionName::AppInfo));
                assert_eq!(raw.warnings.len(), 1, "{:?}", raw.warnings);
            }
        }
    }

    #[test]
    fn zero_page_sections_share_one_budget() {
        // Each all-zero section stores no pages and claims as many pages as lie in the file,
        // which passes the per-section check; together they exceed the file budget.
        let base = build(&Tweak::default());
        let header = decrypt_file_header(&base).unwrap();
        let base_pages = parse_page_map(
            &read_system_page(&base, header.page_map_address, PAGE_MAP_KIND).unwrap(),
        )
        .unwrap()
        .len();
        let mut secs = sections();
        for i in 0..10 {
            secs.push(Sec {
                name: Box::leak(format!("AcDb:Zero{i}").into_boxed_str()),
                data: vec![0; base_pages * 0x7400],
                compressed: true,
                page_size: 0x7400,
            });
        }
        let file = build_with(&secs, &Tweak::default());
        let raw = read_file(&file).unwrap();
        let skipped = raw.warnings.iter().filter(|w| w.contains("budget")).count();
        assert!(skipped > 0, "{:?}", raw.warnings);
        let total: usize = raw.sections.values().map(Vec::len).sum();
        let budget = base_pages * 0x7400 + file.len();
        assert!(total <= budget, "{total:#x} > {budget:#x}");
        // Sections read before the budget ran out are intact.
        assert_eq!(
            raw.sections.get(&SectionName::Other("AcDb:Zero0".into())),
            Some(&vec![0; base_pages * 0x7400]),
            "{:?}",
            raw.warnings
        );
    }

    #[test]
    fn page_shared_by_two_sections_errors() {
        // Section 1 (AcDb:Classes) takes the first page reference of section 0 (AcDb:Header).
        // Patch the section ids so the shared page passes its own section id check.
        let mut secs = sections();
        secs[1].data = secs[0].data.clone();
        let file = build_with(&secs, &Tweak::default());
        let header = decrypt_file_header(&file).unwrap();
        let pages = parse_page_map(
            &read_system_page(&file, header.page_map_address, PAGE_MAP_KIND).unwrap(),
        )
        .unwrap();
        let section_map = read_system_page(
            &file,
            pages[&header.section_map_id].address,
            SECTION_MAP_KIND,
        )
        .unwrap();
        let mut descs = parse_section_map(&section_map).unwrap();
        descs[1].pages = descs[0].pages.clone();
        descs[1].section_id = descs[0].section_id;
        let mut st = SectionReader {
            bytes: &file,
            page_map: &pages,
            in_file_pages: pages.len() as u64,
            budget: u64::MAX,
            used_pages: BTreeSet::new(),
            warnings: Vec::new(),
        };
        assert_eq!(read_section(&mut st, &descs[0]).unwrap(), secs[0].data);
        assert!(matches!(
            read_section(&mut st, &descs[1]),
            Err(Error::Corrupt(m)) if m.contains("already used")
        ));
    }

    #[test]
    fn duplicate_section_id_errors() {
        // Two descriptions sharing a section id cannot both own their pages.
        let mut file = build(&Tweak::default());
        let header = decrypt_file_header(&file).unwrap();
        let pages = parse_page_map(
            &read_system_page(&file, header.page_map_address, PAGE_MAP_KIND).unwrap(),
        )
        .unwrap();
        let at = pages[&header.section_map_id].address as usize;
        let mut map = read_system_page(&file, at as u64, SECTION_MAP_KIND).unwrap();
        // Section 1's description follows section 0's description and its one page reference.
        let id_at = SECTION_MAP_HEADER_LEN + SECTION_DESC_LEN + PAGE_REF_LEN + 0x18;
        map[id_at..id_at + 4].copy_from_slice(&1u32.to_le_bytes());
        // Rewrite the section map page in place; it shrinks or keeps its size.
        let mut page = Vec::new();
        system_page(&mut page, SECTION_MAP_KIND, &map);
        assert!(page.len() as u64 <= pages[&header.section_map_id].size);
        file[at..at + page.len()].copy_from_slice(&page);
        assert!(matches!(
            read_file(&file),
            Err(Error::Corrupt(m)) if m.contains("duplicate section id")
        ));
    }

    #[test]
    fn bad_page_map_entry_sizes_error() {
        for size in [0, 0x10, 0x21] {
            let t = Tweak {
                extra_entries: vec![(400, size)],
                ..Tweak::default()
            };
            assert!(
                matches!(read_file(&build(&t)), Err(Error::Corrupt(_))),
                "{size:#x}"
            );
        }
    }

    #[test]
    fn out_of_file_entries_do_not_raise_the_budget() {
        // 150 page-map entries past EOF, plus one all-zero section claiming 100 pages more than
        // the file holds: only pages inside the file count, so the section is rejected.
        let base = build(&Tweak::default());
        let header = decrypt_file_header(&base).unwrap();
        let in_file = parse_page_map(
            &read_system_page(&base, header.page_map_address, PAGE_MAP_KIND).unwrap(),
        )
        .unwrap()
        .len();
        let mut secs = sections();
        secs.push(Sec {
            name: "AcDb:Zero",
            data: vec![0; (in_file + 100) * 0x7400],
            compressed: true,
            page_size: 0x7400,
        });
        let t = Tweak {
            extra_entries: (1000..1150).map(|n| (n, 0x1000_0000)).collect(),
            ..Tweak::default()
        };
        let raw = read_file(&build_with(&secs, &t)).unwrap();
        assert!(
            !raw.sections
                .contains_key(&SectionName::Other("AcDb:Zero".into()))
        );
        assert!(
            raw.warnings.iter().any(|w| w.contains("more pages")),
            "{:?}",
            raw.warnings
        );
    }

    #[test]
    fn header_page_size_must_match_page_map() {
        let t = Tweak {
            first_header_page_size: Some(0x20),
            ..Tweak::default()
        };
        assert!(matches!(
            read_file(&build(&t)),
            Err(Error::Corrupt(m)) if m.contains("header page size")
        ));
    }

    #[test]
    fn short_stored_page_holding_its_data_is_read() {
        // AppInfo is stored in 0x40-byte pages while the section map says 0x80: the payload is
        // shorter than the page, which ODS 4.5 calls invalid, but it holds the section's 8
        // bytes, so it is read, with a warning.
        let mut secs = sections();
        secs[4].page_size = 0x40;
        let t = Tweak {
            max_page: Some((4, 0x80)),
            ..Tweak::default()
        };
        let raw = read_file(&build_with(&secs, &t)).unwrap();
        assert_eq!(raw.sections[&SectionName::AppInfo], b"app info");
        assert!(
            raw.warnings.iter().any(|w| w.contains("shorter")),
            "{:?}",
            raw.warnings
        );
    }

    #[test]
    fn short_stored_page_missing_data_is_skipped() {
        // A 0x40-byte stored page for a 0x50-byte section whose map says 0x80: data is
        // missing, so the (optional) section is skipped.
        let mut secs = sections();
        secs[4].page_size = 0x40;
        secs[4].data = vec![7; 0x40];
        let t = Tweak {
            max_page: Some((4, 0x80)),
            size: Some((4, 0x50)),
            ..Tweak::default()
        };
        let raw = read_file(&build_with(&secs, &t)).unwrap();
        assert!(!raw.sections.contains_key(&SectionName::AppInfo));
        assert!(
            raw.warnings
                .iter()
                .any(|w| w.contains("AcDb:AppInfo") && w.contains("bytes of data it holds")),
            "{:?}",
            raw.warnings
        );
    }

    #[test]
    fn skipped_optional_section_still_counts_for_duplicates() {
        let mut secs = sections();
        secs.push(Sec {
            name: "AcDb:AppInfo",
            data: b"again".to_vec(),
            compressed: false,
            page_size: 0x80,
        });
        let t = Tweak {
            size: Some((4, u64::MAX)), // first AppInfo is skipped
            ..Tweak::default()
        };
        assert!(matches!(
            read_file(&build_with(&secs, &t)),
            Err(Error::Corrupt(m)) if m.contains("duplicate section")
        ));
    }

    #[test]
    fn optional_section_failure_is_a_warning() {
        let t = Tweak {
            size: Some((4, u64::MAX)),
            ..Tweak::default()
        };
        let raw = read_file(&build(&t)).unwrap();
        assert!(!raw.sections.contains_key(&SectionName::AppInfo));
        assert!(
            raw.warnings[0].contains("AcDb:AppInfo"),
            "{:?}",
            raw.warnings
        );
    }

    #[test]
    fn encrypted_required_section_is_encrypted_error() {
        let t = Tweak {
            encrypted: Some(0),
            ..Tweak::default()
        };
        assert!(matches!(
            read_file(&build(&t)),
            Err(Error::Encrypted(name)) if name == "AcDb:Header"
        ));
    }

    #[test]
    fn encrypted_optional_section_is_a_warning() {
        let t = Tweak {
            encrypted: Some(4),
            ..Tweak::default()
        };
        let raw = read_file(&build(&t)).unwrap();
        assert!(!raw.sections.contains_key(&SectionName::AppInfo));
        assert!(raw.warnings[0].contains("encrypted"), "{:?}", raw.warnings);
    }

    #[test]
    fn data_page_checksum_is_a_warning_system_page_is_an_error() {
        let file = build(&Tweak::default());
        // Uncompressed AppInfo payload: a flipped byte changes the data but still reads.
        let at = file.windows(8).position(|w| w == b"app info").unwrap();
        let mut bad = file.clone();
        bad[at] ^= 0x20;
        let raw = read_file(&bad).unwrap();
        assert_eq!(raw.warnings.len(), 1, "{:?}", raw.warnings);
        assert!(raw.sections[&SectionName::AppInfo].starts_with(b"App info"));

        // The page map is the last system page, followed by the repeated header data.
        let page_map_at = file.len() - ENCRYPTED_LEN - 0x20;
        let page_map_at = (0..=page_map_at)
            .rev()
            .step_by(0x20)
            .find(|&a| u32_at(&file, a, "").unwrap() == PAGE_MAP_KIND)
            .unwrap();
        let mut bad = file.clone();
        bad[page_map_at + SYSTEM_HEADER_LEN] ^= 1;
        assert!(matches!(read_file(&bad), Err(Error::BadCrc { .. })));
    }

    /// Without its trailing header copy and with an optional section unreadable, the file
    /// is `Truncated`; with the copy, the section is skipped with a warning.
    #[test]
    fn missing_copy_and_section_is_truncated() {
        let file = build(&Tweak {
            size: Some((4, u64::MAX)),
            ..Tweak::default()
        });
        assert!(read_file(&file).is_ok());
        let cut = &file[..file.len() - ENCRYPTED_LEN];
        assert!(matches!(
            read_file(cut),
            Err(Error::Truncated { context }) if context.contains("repeated header")
        ));
    }

    /// A prefix that cuts into the data fails; one that holds every section (lacking only
    /// padding and the trailing header copy) reads them all, with a warning.
    #[test]
    fn every_prefix_errors_or_warns() {
        let file = build(&Tweak::default());
        let whole = read_file(&file).unwrap();
        let mut read_short = 0;
        for n in 0..file.len() {
            if let Ok(raw) = read_file(&file[..n]) {
                assert_eq!(raw.sections, whole.sections, "prefix {n:#x}");
                assert!(
                    raw.warnings
                        .iter()
                        .any(|w| w.contains("repeated R2004 file header missing")),
                    "prefix {n:#x}: {:?}",
                    raw.warnings
                );
                read_short += 1;
            }
        }
        assert!(read_short >= ENCRYPTED_LEN, "{read_short}");
    }

    #[test]
    fn every_byte_flip_is_handled() {
        let file = build(&Tweak::default());
        for i in 0..file.len() {
            let mut bad = file.clone();
            bad[i] ^= 0xFF;
            let _ = read_file(&bad); // must not panic
        }
    }

    #[test]
    fn duplicate_section_errors() {
        let mut secs = sections();
        secs.push(Sec {
            name: "AcDb:Header",
            data: vec![1],
            compressed: true,
            page_size: 0x7400,
        });
        assert!(matches!(
            read_file(&build_with(&secs, &Tweak::default())),
            Err(Error::Corrupt(_))
        ));
    }

    #[test]
    fn missing_required_section_errors() {
        let secs: Vec<Sec> = sections()
            .into_iter()
            .filter(|s| s.name != "AcDb:Handles")
            .collect();
        assert!(matches!(
            read_file(&build_with(&secs, &Tweak::default())),
            Err(Error::Corrupt(m)) if m.contains("AcDb:Handles")
        ));
    }
}
