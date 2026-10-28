//! R2018 (AC1032) container writer, the inverse of the R2004-family reader (`r2004.rs`),
//! ODS chapter 4. ODS chapter 8 says the R2018 container is structurally the R2004 one.
//!
//! The file, in order:
//! 1. The 0x100-byte file header (ODS 4.1): plain fields at 0x00-0x80, the encrypted 0x6C-byte
//!    header data at 0x80, then keystream bytes 0xEC-0x100.
//! 2. Data pages from 0x100, section by section in stream order (ODS 4.5): SummaryInfo,
//!    Preview, VBAProject, AppInfo, AppInfoHistory, FileDepList, AcDsPrototype_1b, RevHistory,
//!    Security, AcDbObjects, ObjFreeSpace, Template, Handles, Classes, AuxHeader, Header,
//!    Signature, then sections without a rule ([`SectionName::Other`]) by name.
//! 3. The section map, then the section page map: system pages (ODS 4.3), each followed by a
//!    copy of its header with the sizes and checksum zeroed.
//! 4. The repeated header data (encrypted, without the keystream tail), ending the file.
//!
//! Where ODS is silent or differs, the layout follows the AutoCAD 2018 files of the corpus:
//! - Section map order: the empty section, AcDsPrototype_1b, Security, FileDepList,
//!   VBAProject, AppInfoHistory, AppInfo, Preview, SummaryInfo, RevHistory, AcDbObjects,
//!   ObjFreeSpace, Template, Handles, Classes, AuxHeader, Header, Signature, then the others.
//!   Ids follow map position (ODS 4.5): 0 for the empty section, then descending to 1.
//! - Stored (uncompressed) sections are one page of their size rounded up to 0x80, or 0x400
//!   for Preview, with zero padding. ODS's table gives SummaryInfo 0x100 pages; AutoCAD writes
//!   0x80. Security keeps ODS's 0x7400; VBAProject is its size plus 0x80, rounded to 0x20.
//! - Every known section is written with the encrypted flag 0 (ODS says AppInfo is 2) and the
//!   unknown field at description offset 0x10 set to 1. Sections the corpus has no example of
//!   (Signature, Security, VBAProject, FileDepList) and [`SectionName::Other`] sections take
//!   their flags and page size from [`RawSections::section_meta`] when it has them.
//! - Every page is stored, all-zero pages included (ODS 4.5 allows omitting them; AutoCAD 2018
//!   does not).
//! - Data pages are numbered 1..=N in file order; the section map is N+3 and the page map N+4
//!   (ODS 4.4 says the gap is 1; AutoCAD leaves two numbers unused).
//! - The page map's extent covers the repeated header data, so its end (file header 0x2C) is
//!   past the end of the file, as in AutoCAD's files. No gaps are written.
//! - Header fields ODS leaves as zeros at 0x2C and 0x30 hold the AppInfo and AppInfoHistory
//!   addresses (page address + 0x20, like the summary info address). The undocumented longs
//!   at 0x34-0x44 are kept from an R2018 [`RawSections::file_header`], else AutoCAD 2018's
//!   values. The security flags and the unknown long at 0x18-0x20 are written as 0: no
//!   section is encrypted or signed.
//! - Bytes between the end of a page's contents and the next 0x20 boundary are keystream bytes
//!   from its start (ODS 4.3, 4.6); padding inside a stored page is zero.

use super::r2004::{
    COMPRESSED_PAGE_MAX, DATA_HEADER_LEN, DATA_PAGE_KIND, ENCRYPTED_AT, ENCRYPTED_LEN, FILE_ID,
    OFF_CODEPAGE, OFF_MAINTENANCE, OFF_PREVIEW, PAGE_BASE, PAGE_MAP_KIND, SECTION_MAP_KIND,
    SYSTEM_HEADER_LEN,
};
use super::{R2004_PLAIN_HEADER_LEN, RawSections, SectionMeta, SectionName, check_required};
use crate::codec::{crc32, lz77_2004, page_checksum, page_header_mask, xor_r2004_file_header};
use crate::error::{Error, Result};
use crate::version::Version;

/// Plain header offsets written from the layout (ODS 4.1; 0x2C and 0x30 from the corpus).
const OFF_SUMMARY_INFO: usize = 0x20;
const OFF_VBA_PROJECT: usize = 0x24;
const OFF_0X80: usize = 0x28;
const OFF_APP_INFO: usize = 0x2C;
const OFF_APP_INFO_HISTORY: usize = 0x30;
/// A section name is stored NUL-terminated in 64 bytes (ODS 4.5).
const NAME_LEN: usize = 64;
/// Page numbers are positive `i32` values in the page map (negative numbers are gaps).
const MAX_PAGE_NUMBER: u32 = i32::MAX as u32;
/// Largest stored page or data size: its page extent (plus the 0x20 header, rounded up to
/// 0x20) must fit the page map's 32-bit size.
const MAX_PAGE_DATA: u64 = u32::MAX as u64 - 0x40;

/// Plain header used when [`RawSections::file_header`] is not an R2018 one, field by field
/// (ODS 4.1); the fields the writer derives (version string, maintenance version, preview,
/// code page, section addresses, ...) are written over it. Where ODS gives no value, the value
/// is that of AutoCAD 2018's file (acadsharp-sample-ac1032).
const DEFAULT_PLAIN_HEADER: [u8; R2004_PLAIN_HEADER_LEN] = {
    /// 0x0C: ODS "byte 0x00, 0x01, or 0x03"; AutoCAD 2018 writes 3.
    const FLAGS_0X0C: u8 = 3;
    /// 0x11, 0x12: the application version and maintenance version that wrote the file
    /// (33 = AC1032, as the AuxHeader numbers versions; maintenance 0).
    const APP_VERSION: u8 = 33;
    const APP_MAINTENANCE: u8 = 0;
    /// 0x34-0x43: four longs ODS does not describe; AutoCAD 2018's file has 0x202, 0x17B,
    /// 0x149, 0x149 (0x149 is also its AuxHeader maintenance version). Another AutoCAD build
    /// writes 0xFF, 0xCD, 0xCD at 0x38-0x44, so nothing is derived from them.
    const UNDOCUMENTED_LONGS: [u32; 4] = [0x202, 0x17B, 0x149, 0x149];
    let mut h = [0u8; R2004_PLAIN_HEADER_LEN];
    h[0x0C] = FLAGS_0X0C;
    h[0x11] = APP_VERSION;
    h[0x12] = APP_MAINTENANCE;
    let mut i = 0;
    while i < UNDOCUMENTED_LONGS.len() {
        let b = UNDOCUMENTED_LONGS[i].to_le_bytes();
        let at = 0x34 + 4 * i;
        h[at] = b[0];
        h[at + 1] = b[1];
        h[at + 2] = b[2];
        h[at + 3] = b[3];
        i += 1;
    }
    h
};
/// Security flags (0x18) and the unknown long after them (ODS 4.1).
const SECURITY_FIELDS: std::ops::Range<usize> = 0x18..0x20;

/// How a section is paged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PageRule {
    /// Pages of exactly this size.
    Fixed(u32),
    /// One page: the section size rounded up to a multiple of this (at least one multiple).
    RoundUp(u32),
    /// One page: the section size plus 0x80, rounded up to 0x20 (ODS 4.5, VBAProject).
    PlusHeader,
}

/// One section as it will be written.
struct Plan<'a> {
    name: &'a SectionName,
    label: &'a str,
    data: &'a [u8],
    compressed: bool,
    encrypted: u32,
    unknown: u32,
    page_size: u32,
    section_id: u32,
}

/// Section map position (ODS 4.5 table merged with the corpus order).
fn map_rank(name: &SectionName) -> u8 {
    use SectionName::*;
    match name {
        Other(n) if n.is_empty() => 0,
        AcDsPrototype => 1,
        Security => 2,
        FileDepList => 3,
        VbaProject => 4,
        AppInfoHistory => 5,
        AppInfo => 6,
        Preview => 7,
        SummaryInfo => 8,
        RevHistory => 9,
        Objects => 10,
        ObjFreeSpace => 11,
        Template => 12,
        Handles => 13,
        Classes => 14,
        AuxHeader => 15,
        Header => 16,
        Signature => 17,
        Other(_) => 18,
    }
}

/// Position of a section's pages in the file (ODS 4.5 stream order merged with the corpus).
fn stream_rank(name: &SectionName) -> u8 {
    use SectionName::*;
    match name {
        SummaryInfo => 0,
        Preview => 1,
        VbaProject => 2,
        AppInfo => 3,
        AppInfoHistory => 4,
        FileDepList => 5,
        AcDsPrototype => 6,
        RevHistory => 7,
        Security => 8,
        Objects => 9,
        ObjFreeSpace => 10,
        Template => 11,
        Handles => 12,
        Classes => 13,
        AuxHeader => 14,
        Header => 15,
        Signature => 16,
        Other(_) => 17,
    }
}

/// Compression and paging of a known section (ODS 4.5 table, corrected by the corpus), or
/// `None` for [`SectionName::Other`].
fn known_rule(name: &SectionName) -> Option<(bool, PageRule)> {
    use SectionName::*;
    Some(match name {
        Security => (false, PageRule::Fixed(COMPRESSED_PAGE_MAX)),
        FileDepList | AppInfo | AppInfoHistory | SummaryInfo => (false, PageRule::RoundUp(0x80)),
        Preview => (false, PageRule::RoundUp(0x400)),
        VbaProject => (false, PageRule::PlusHeader),
        Other(_) => return None,
        _ => (true, PageRule::Fixed(COMPRESSED_PAGE_MAX)),
    })
}

fn too_large(label: &str, len: u64) -> Error {
    Error::Encode(format!(
        "section {label:?}: {len:#x} bytes is too large for an R2018 page"
    ))
}

/// Page size of a section of `len` bytes under `rule`.
fn page_size(label: &str, len: u64, rule: PageRule) -> Result<u32> {
    let round_up = |n: u64, g: u64| n.div_ceil(g).checked_mul(g);
    let size = match rule {
        PageRule::Fixed(p) => Some(u64::from(p)),
        PageRule::RoundUp(g) => round_up(len.max(1), u64::from(g)),
        PageRule::PlusHeader => len.checked_add(0x80).and_then(|n| round_up(n, 0x20)),
    };
    size.and_then(|s| u32::try_from(s).ok())
        .filter(|&s| s > 0 && u64::from(s) <= MAX_PAGE_DATA)
        .ok_or_else(|| too_large(label, len))
}

/// Known sections the corpus has no example of: their read metadata is preferred to the
/// ODS-derived rule.
fn uncharted(name: &SectionName) -> bool {
    use SectionName::*;
    matches!(name, Signature | Security | VbaProject | FileDepList)
}

/// Compressed flag, encrypted flag, unknown field and page size from read metadata, or the
/// defaults (compressed 0x7400 pages, not encrypted, unknown field 1) without it. Metadata
/// the reader would refuse is an error. A stored page size above both 0x7400 and the data
/// rounded up to 0x80 is cut to the latter, so a tiny section cannot demand a huge page.
fn meta_rule(label: &str, len: u64, meta: Option<&SectionMeta>) -> Result<(bool, u32, u32, u32)> {
    let Some(m) = meta else {
        return Ok((true, 0, 1, COMPRESSED_PAGE_MAX));
    };
    let bad = |what: String| Err(Error::Encode(format!("section {label:?}: {what}")));
    if m.encrypted == 1 {
        return bad("password-encrypted sections cannot be written".into());
    }
    let p = m.max_page_size;
    if p == 0 || (m.compressed && p > COMPRESSED_PAGE_MAX) {
        return bad(format!("invalid page size {p:#x}"));
    }
    let rule = if m.compressed {
        PageRule::Fixed(p)
    } else {
        let fitted = page_size(label, len, PageRule::RoundUp(0x80))?;
        if p > COMPRESSED_PAGE_MAX && p > fitted {
            PageRule::Fixed(fitted)
        } else {
            PageRule::Fixed(p)
        }
    };
    Ok((
        m.compressed,
        m.encrypted,
        m.unknown,
        page_size(label, len, rule)?,
    ))
}

/// Physical extent of a page holding `content` bytes: rounded up to 0x20 (ODS 4.3, 4.6).
fn extent(content: u64) -> u64 {
    content.div_ceil(0x20) * 0x20
}

/// Compression, encryption, unknown field and page size of every section, in section map order
/// with section ids assigned.
fn plan(raw: &RawSections) -> Result<Vec<Plan<'_>>> {
    const EMPTY: &SectionName = &SectionName::Other(String::new());
    let mut plans = Vec::with_capacity(raw.sections.len() + 1);
    if !raw.sections.contains_key(EMPTY) {
        // ODS 4.5: the first section of the map is the empty one, id 0.
        plans.push(Plan {
            name: EMPTY,
            label: "",
            data: &[],
            compressed: true,
            encrypted: 0,
            unknown: 1,
            page_size: COMPRESSED_PAGE_MAX,
            section_id: 0,
        });
    }
    for (name, data) in &raw.sections {
        let label = name.r2004_name();
        if SectionName::from_r2004_name(label) != *name {
            return Err(Error::Encode(format!(
                "section name {label:?} is reserved for a known section"
            )));
        }
        if label.len() >= NAME_LEN || label.contains('\0') {
            return Err(Error::Encode(format!(
                "section name {label:?} does not fit the section map"
            )));
        }
        let len = data.len() as u64;
        let meta = raw.section_meta.get(name);
        let (compressed, encrypted, unknown, page_size) = match known_rule(name) {
            Some((compressed, rule)) if !(meta.is_some() && uncharted(name)) => {
                (compressed, 0, 1, page_size(label, len, rule)?)
            }
            _ => meta_rule(label, len, meta)?,
        };
        plans.push(Plan {
            name,
            label,
            data,
            compressed,
            encrypted,
            unknown,
            page_size,
            section_id: 0,
        });
    }
    // Stable: unknown sections keep their name order.
    plans.sort_by_key(|p| map_rank(p.name));
    let count = u32::try_from(plans.len())
        .map_err(|_| Error::Encode("too many sections to write".into()))?;
    for (i, p) in plans.iter_mut().enumerate().skip(1) {
        p.section_id = count - i as u32;
    }
    Ok(plans)
}

/// The header keystream (ODS 4.1), which also fills the bytes between pages (ODS 4.3, 4.6).
fn keystream() -> [u8; 0x100] {
    let mut k = [0u8; 0x100];
    xor_r2004_file_header(&mut k);
    k
}

fn put32(b: &mut Vec<u8>, v: u32) {
    b.extend_from_slice(&v.to_le_bytes());
}

fn put64(b: &mut Vec<u8>, v: u64) {
    b.extend_from_slice(&v.to_le_bytes());
}

/// A system page (ODS 4.3) holding `data`: header, compressed data and the header copy with
/// sizes and checksum zeroed. Not padded.
fn system_page(kind: u32, data: &[u8]) -> Result<Vec<u8>> {
    let comp = lz77_2004::compress(data);
    let sizes = u32::try_from(data.len())
        .ok()
        .zip(u32::try_from(comp.len()).ok())
        .ok_or_else(|| Error::Encode("system page too large".into()))?;
    let mut page = Vec::with_capacity(2 * SYSTEM_HEADER_LEN + comp.len());
    for v in [kind, sizes.0, sizes.1, 2, 0] {
        put32(&mut page, v);
    }
    let sum = page_checksum(page_checksum(0, &page), &comp);
    page[0x10..SYSTEM_HEADER_LEN].copy_from_slice(&sum.to_le_bytes());
    page.extend_from_slice(&comp);
    for v in [kind, 0, 0, 2, 0] {
        put32(&mut page, v);
    }
    Ok(page)
}

/// Write `sections` (already-encoded R2018 section buffers) as an AC1032 file.
///
/// [`RawSections::warnings`] and [`RawSections::objects_base`] are not used.
/// [`RawSections::file_header`] is used only when it is an R2018 header (magic `AC1032`);
/// otherwise AutoCAD 2018's header is written. [`RawSections::maintenance`] goes to 0x0B
/// unchanged: AutoCAD 2018 writes 0 there (all AC1032 corpus files), so a drawing converted
/// from another version should set 0. The R2018 Header section has its extra long whatever
/// this value is (ODS chapter 8), so the value does not change how sections are encoded. The preview's
/// address in the file header is where the Preview section lands; when
/// [`RawSections::preview_offset`] is set and differs (the preview holds absolute addresses,
/// see [`r2018_preview_address`]), this is an error. Sections that cannot be written (a name
/// reserved for a known section or too long, a password-encrypted section, sizes beyond the
/// format's 32-bit page fields) are errors too. Sections of another version are
/// [`Error::Encode`].
pub fn write_r2018(raw: &RawSections) -> Result<Vec<u8>> {
    if raw.version != Version::R2018 {
        // Sections of another version are a value the writer cannot encode,
        // not a file of a version the library cannot read.
        return Err(Error::Encode(format!(
            "sections of {} (the R2018 writer takes R2018 sections)",
            String::from_utf8_lossy(raw.version.magic())
        )));
    }
    check_required(&raw.sections).map_err(|e| match e {
        Error::Corrupt(m) => Error::Encode(m),
        e => e,
    })?;
    let plans = plan(raw)?;
    let keystream = keystream();
    let mut order: Vec<usize> = (0..plans.len()).collect();
    order.sort_by_key(|&i| stream_rank(plans[i].name));

    let mut file = vec![0u8; PAGE_BASE as usize];
    // (page number, extent) in file order, and each section's page references.
    let mut entries: Vec<(u32, u32)> = Vec::new();
    let mut refs: Vec<Vec<(u32, u32, u64)>> = vec![Vec::new(); plans.len()];
    let mut first_page: Vec<Option<u64>> = vec![None; plans.len()];
    let mut number = 0u32;
    for &i in &order {
        let p = &plans[i];
        let page_size = p.page_size as usize;
        for (k, chunk) in p.data.chunks(page_size).enumerate() {
            let mut page = chunk.to_vec();
            page.resize(page_size, 0);
            let payload = if p.compressed {
                lz77_2004::compress(&page)
            } else {
                page
            };
            number = number
                .checked_add(1)
                .filter(|&n| n <= MAX_PAGE_NUMBER - 4)
                .ok_or_else(|| Error::Encode("too many pages to write".into()))?;
            let address = file.len() as u64;
            let data_size = u32::try_from(payload.len())
                .ok()
                .filter(|&d| u64::from(d) <= MAX_PAGE_DATA)
                .ok_or_else(|| too_large(p.label, p.data.len() as u64))?;
            let page_extent = extent(DATA_HEADER_LEN as u64 + u64::from(data_size)) as u32;
            let start = (k * page_size) as u64;

            // ODS 4.6: data checksum with seed 0, then the header (checksum field zeroed)
            // seeded with it; every header word is masked with the page address.
            let data_sum = page_checksum(0, &payload);
            let mut header = Vec::with_capacity(DATA_HEADER_LEN);
            for v in [DATA_PAGE_KIND, p.section_id, data_size, page_extent] {
                put32(&mut header, v);
            }
            put64(&mut header, start);
            put32(&mut header, 0);
            put32(&mut header, data_sum);
            let header_sum = page_checksum(data_sum, &header);
            header[0x18..0x1C].copy_from_slice(&header_sum.to_le_bytes());
            let mask = page_header_mask(address);
            for w in header.chunks_exact(4) {
                put32(
                    &mut file,
                    u32::from_le_bytes([w[0], w[1], w[2], w[3]]) ^ mask,
                );
            }
            file.extend_from_slice(&payload);
            let fill = (address + u64::from(page_extent)) as usize - file.len();
            file.extend_from_slice(&keystream[..fill]);

            entries.push((number, page_extent));
            refs[i].push((number, data_size, start));
            first_page[i].get_or_insert(address);
        }
    }

    // Section map (ODS 4.5), in map order.
    let count = plans.len() as u32;
    let mut map = Vec::new();
    for v in [count, 2, COMPRESSED_PAGE_MAX, 0, count] {
        put32(&mut map, v);
    }
    for (p, refs) in plans.iter().zip(&refs) {
        put64(&mut map, p.data.len() as u64);
        put32(&mut map, refs.len() as u32);
        put32(&mut map, p.page_size);
        put32(&mut map, p.unknown);
        put32(&mut map, if p.compressed { 2 } else { 1 });
        put32(&mut map, p.section_id);
        put32(&mut map, p.encrypted);
        let mut name = [0u8; NAME_LEN];
        name[..p.label.len()].copy_from_slice(p.label.as_bytes());
        map.extend_from_slice(&name);
        for &(n, size, start) in refs {
            put32(&mut map, n);
            put32(&mut map, size);
            put64(&mut map, start);
        }
    }
    let section_map_id = number + 3;
    let page_map_id = number + 4;
    let section_map = system_page(SECTION_MAP_KIND, &map)?;
    let section_map_extent = u32::try_from(extent(section_map.len() as u64))
        .map_err(|_| Error::Encode("section map too large".into()))?;
    file.extend_from_slice(&section_map);
    file.extend_from_slice(&keystream[..section_map_extent as usize - section_map.len()]);
    entries.push((section_map_id, section_map_extent));

    // Section page map (ODS 4.4). Its own entry's size is its extent, which depends on its
    // compressed size: grow the extent until it holds the page and the repeated header data.
    // Each round only changes that one entry, so this settles after two or three rounds.
    let page_map_at = file.len() as u64;
    let mut page_map_extent = 0u32;
    let mut page_map = None;
    for _ in 0..16 {
        let mut data = Vec::with_capacity((entries.len() + 1) * 8);
        for &(n, size) in entries.iter().chain([&(page_map_id, page_map_extent)]) {
            put32(&mut data, n);
            put32(&mut data, size);
        }
        let page = system_page(PAGE_MAP_KIND, &data)?;
        let needed = u32::try_from(extent((page.len() + ENCRYPTED_LEN) as u64))
            .map_err(|_| Error::Encode("section page map too large".into()))?;
        if needed <= page_map_extent {
            page_map = Some(page);
            break;
        }
        page_map_extent = needed;
    }
    let page_map =
        page_map.ok_or_else(|| Error::Encode("section page map size does not settle".into()))?;
    file.extend_from_slice(&page_map);
    entries.push((page_map_id, page_map_extent));
    let page_map_extent = u64::from(page_map_extent);
    let second_header_at = file.len() as u64;

    // File header data (ODS 4.1).
    let mut data = Vec::with_capacity(ENCRYPTED_LEN);
    data.extend_from_slice(&FILE_ID);
    // 0x0C-0x2C: 0, 0x6C, 4, the three tree node gaps (no gaps are written), 1 and the last
    // page id.
    for v in [0, 0x6C, 4, 0, 0, 0, 1, page_map_id] {
        put32(&mut data, v);
    }
    put64(&mut data, page_map_at + page_map_extent - PAGE_BASE);
    put64(&mut data, second_header_at);
    // Gap amount, section page amount, 0x20, 0x80, 0x40, page map id.
    for v in [0, entries.len() as u32, 0x20, 0x80, 0x40, page_map_id] {
        put32(&mut data, v);
    }
    put64(&mut data, page_map_at - PAGE_BASE);
    // Section map id, section page array size, gap array size, CRC placeholder.
    for v in [section_map_id, page_map_id, 0, 0] {
        put32(&mut data, v);
    }
    let crc = crc32(0, &data);
    data[0x68..].copy_from_slice(&crc.to_le_bytes());
    xor_r2004_file_header(&mut data);
    file.extend_from_slice(&data);

    // Plain header (ODS 4.1).
    let address = |name: &SectionName| -> Result<u32> {
        let at = plans
            .iter()
            .position(|p| p.name == name)
            .and_then(|i| first_page[i])
            .map_or(0, |a| a + DATA_HEADER_LEN as u64);
        u32::try_from(at).map_err(|_| {
            Error::Encode(format!(
                "section {:?} lies beyond the 32-bit header address",
                name.r2004_name()
            ))
        })
    };
    let preview = address(&SectionName::Preview)?;
    if let Some(want) = raw
        .preview_offset
        .filter(|&want| want != u64::from(preview))
    {
        return Err(Error::Encode(if preview == 0 {
            format!("preview address {want:#x} set but there is no preview page")
        } else {
            format!("the preview was encoded for address {want:#x} but lands at {preview:#x}")
        }));
    }
    let mut plain = raw
        .file_header
        .filter(|h| h[..6] == *Version::R2018.magic())
        .unwrap_or(DEFAULT_PLAIN_HEADER);
    plain[SECURITY_FIELDS].fill(0);
    plain[..6].copy_from_slice(Version::R2018.magic());
    plain[6..OFF_MAINTENANCE].fill(0);
    plain[OFF_MAINTENANCE] = raw.maintenance;
    plain[OFF_CODEPAGE..OFF_CODEPAGE + 2].copy_from_slice(&raw.codepage.to_le_bytes());
    plain[OFF_CODEPAGE + 2..0x18].fill(0);
    for (at, v) in [
        (OFF_PREVIEW, preview),
        (OFF_SUMMARY_INFO, address(&SectionName::SummaryInfo)?),
        (OFF_VBA_PROJECT, address(&SectionName::VbaProject)?),
        (OFF_0X80, 0x80),
        (OFF_APP_INFO, address(&SectionName::AppInfo)?),
        (OFF_APP_INFO_HISTORY, address(&SectionName::AppInfoHistory)?),
    ] {
        plain[at..at + 4].copy_from_slice(&v.to_le_bytes());
    }
    file[..R2004_PLAIN_HEADER_LEN].copy_from_slice(&plain);
    // The same encrypted header data, followed by the keystream tail (ODS 4.1).
    file[ENCRYPTED_AT..ENCRYPTED_AT + ENCRYPTED_LEN].copy_from_slice(&data);
    file[ENCRYPTED_AT + ENCRYPTED_LEN..PAGE_BASE as usize]
        .copy_from_slice(&keystream[ENCRYPTED_AT + ENCRYPTED_LEN..]);
    Ok(file)
}

/// File address [`write_r2018`] gives the Preview section (its page address plus 0x20, the
/// value of the header's preview address). The preview holds absolute addresses, so it must be
/// encoded for this address before it is written. Only the SummaryInfo section, written first,
/// comes before the preview; `summary_info_len` is its size, or `None` without one.
pub fn r2018_preview_address(summary_info_len: Option<usize>) -> Result<u64> {
    let len = summary_info_len.map_or(0, |l| l as u64);
    let pages = if len == 0 {
        0
    } else {
        let size = page_size("AcDb:SummaryInfo", len, PageRule::RoundUp(0x80))?;
        extent(DATA_HEADER_LEN as u64 + u64::from(size))
    };
    Ok(PAGE_BASE + pages + DATA_HEADER_LEN as u64)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::container::r2004::{
        SECTION_MAP_HEADER_LEN, SectionDesc, decrypt_file_header, parse_section_map,
        read_system_page,
    };
    use crate::container::{SectionMeta, read, u32_at, u64_at};

    /// One section page map entry (ODS 4.4) with its address; gaps have a negative number.
    #[derive(Debug, Clone, Copy)]
    struct Entry {
        number: i32,
        size: u32,
        address: u64,
    }

    /// The structure of an R2004-family file, as stored.
    struct Layout {
        len: u64,
        plain: [u8; R2004_PLAIN_HEADER_LEN],
        tail: Vec<u8>,
        /// Decrypted header data and its repeated copy.
        data: [u8; ENCRYPTED_LEN],
        second: Vec<u8>,
        entries: Vec<Entry>,
        page_map_at: u64,
        section_map_at: u64,
        section_map_head: Vec<u32>,
        descs: Vec<SectionDesc>,
    }

    impl Layout {
        fn field(&self, at: usize) -> u32 {
            u32_at(&self.data, at, "").unwrap()
        }

        fn field64(&self, at: usize) -> u64 {
            u64_at(&self.data, at, "").unwrap()
        }

        fn page(&self, number: u32) -> Entry {
            *self
                .entries
                .iter()
                .find(|e| e.number == number as i32)
                .unwrap()
        }

        /// Section names in the file order of their first pages.
        fn stream_order(&self) -> Vec<String> {
            let mut firsts: Vec<(u64, String)> = self
                .descs
                .iter()
                .filter_map(|d| {
                    let at = d.pages.iter().map(|r| self.page(r.number).address).min()?;
                    Some((at, d.name.clone()))
                })
                .collect();
            firsts.sort();
            firsts.into_iter().map(|f| f.1).collect()
        }

        /// File header address of a section: its first page plus 0x20, or 0 without one.
        fn address_of(&self, name: &str) -> u32 {
            self.descs
                .iter()
                .find(|d| d.name == name)
                .and_then(|d| d.pages.first())
                .map_or(0, |r| self.page(r.number).address as u32 + 0x20)
        }
    }

    fn layout(bytes: &[u8]) -> Layout {
        let header = decrypt_file_header(bytes).unwrap();
        let mut second = bytes[header.second_header_address as usize..].to_vec();
        xor_r2004_file_header(&mut second);
        let map = read_system_page(bytes, header.page_map_address, PAGE_MAP_KIND).unwrap();
        let mut entries = Vec::new();
        let (mut pos, mut address) = (0, PAGE_BASE);
        while pos < map.len() {
            let number = u32_at(&map, pos, "").unwrap() as i32;
            let size = u32_at(&map, pos + 4, "").unwrap();
            entries.push(Entry {
                number,
                size,
                address,
            });
            pos += if number < 0 { 24 } else { 8 };
            address += u64::from(size);
        }
        let section_map_at = entries
            .iter()
            .find(|e| e.number == header.section_map_id as i32)
            .unwrap()
            .address;
        let section_map = read_system_page(bytes, section_map_at, SECTION_MAP_KIND).unwrap();
        Layout {
            len: bytes.len() as u64,
            plain: bytes[..R2004_PLAIN_HEADER_LEN].try_into().unwrap(),
            tail: bytes[ENCRYPTED_AT + ENCRYPTED_LEN..PAGE_BASE as usize].to_vec(),
            data: header.data,
            second,
            entries,
            page_map_at: header.page_map_address,
            section_map_at,
            section_map_head: (0..SECTION_MAP_HEADER_LEN)
                .step_by(4)
                .map(|at| u32_at(&section_map, at, "").unwrap())
                .collect(),
            descs: parse_section_map(&section_map).unwrap(),
        }
    }

    /// Unmasked header words of the data page at `address`.
    fn data_page_header(bytes: &[u8], address: u64) -> Vec<u32> {
        let mask = page_header_mask(address);
        (0..DATA_HEADER_LEN)
            .step_by(4)
            .map(|at| u32_at(bytes, address as usize + at, "").unwrap() ^ mask)
            .collect()
    }

    /// Structural checks that hold for AutoCAD's R2018 files and for ours: the header data
    /// agrees with the page map, system pages carry their header copy and keystream fill, and
    /// data pages are filled and padded like AutoCAD's.
    fn check_invariants(name: &str, bytes: &[u8], l: &Layout) {
        let keystream = keystream();
        assert_eq!(l.tail, keystream[0xEC..], "{name}: header keystream tail");
        assert_eq!(l.second, l.data, "{name}: repeated header data");
        assert_eq!(
            l.field64(0x34) + ENCRYPTED_LEN as u64,
            l.len,
            "{name}: second header"
        );
        // Plain header addresses point at their sections' first pages.
        for (at, section) in [
            (OFF_PREVIEW, "AcDb:Preview"),
            (OFF_SUMMARY_INFO, "AcDb:SummaryInfo"),
            (OFF_VBA_PROJECT, "AcDb:VBAProject"),
            (OFF_APP_INFO, "AcDb:AppInfo"),
            (OFF_APP_INFO_HISTORY, "AcDb:AppInfoHistory"),
        ] {
            let stored = u32_at(&l.plain, at, "").unwrap();
            assert_eq!(stored, l.address_of(section), "{name}: {section} address");
        }

        // The page map is the last page. Without gaps, the section map comes right before it
        // (with gaps, AutoCAD may put a reused page in between).
        let pages: Vec<Entry> = l.entries.iter().filter(|e| e.number > 0).copied().collect();
        let gaps = l.entries.len() - pages.len();
        let pm = *l.entries.last().unwrap();
        let sm = l.page(l.field(0x5C));
        assert_eq!(sm.address, l.section_map_at, "{name}");
        assert_eq!(pm.address, l.page_map_at, "{name}");
        if gaps == 0 {
            assert_eq!(
                sm.address + u64::from(sm.size),
                pm.address,
                "{name}: section map"
            );
        }
        for at in [0x28, 0x50] {
            assert_eq!(
                l.field(at),
                pm.number as u32,
                "{name}: page map id at {at:#x}"
            );
        }
        let highest = pages.iter().map(|e| e.number).max().unwrap();
        assert_eq!(
            l.field(0x60),
            highest as u32,
            "{name}: section page array size"
        );
        assert_eq!(
            l.field(0x40) as usize,
            pages.len(),
            "{name}: section page amount"
        );
        assert_eq!(l.field(0x3C) as usize, gaps, "{name}: gap amount");
        assert_eq!(
            l.field64(0x54) + PAGE_BASE,
            pm.address,
            "{name}: page map address"
        );
        assert_eq!(
            l.field64(0x2C) + PAGE_BASE,
            pm.address + u64::from(pm.size),
            "{name}: last page end address"
        );
        if gaps == 0 {
            for at in [0x18, 0x1C, 0x20, 0x64] {
                assert_eq!(l.field(at), 0, "{name}: gap field {at:#x}");
            }
            // Data pages 1..=N in file order; the section map is N+3, the page map N+4.
            let data_pages = pages.len() as i32 - 2;
            let numbers: Vec<i32> = pages.iter().map(|e| e.number).collect();
            let want: Vec<i32> = (1..=data_pages)
                .chain([data_pages + 3, data_pages + 4])
                .collect();
            assert_eq!(numbers, want, "{name}: page numbers");
        }

        // System pages: compression 2, then a header copy with sizes and checksum zeroed. The
        // section map is filled to its extent with keystream (checked without gaps); the page
        // map is followed by the repeated header data.
        for (at, kind) in [(sm.address, SECTION_MAP_KIND), (pm.address, PAGE_MAP_KIND)] {
            let at = at as usize;
            assert_eq!(u32_at(bytes, at, "").unwrap(), kind, "{name}");
            assert_eq!(u32_at(bytes, at + 0x0C, "").unwrap(), 2, "{name}");
            let end = at + SYSTEM_HEADER_LEN + u32_at(bytes, at + 8, "").unwrap() as usize;
            let mut copy = Vec::new();
            for v in [kind, 0, 0, 2, 0] {
                put32(&mut copy, v);
            }
            assert_eq!(
                bytes[end..end + SYSTEM_HEADER_LEN],
                copy,
                "{name}: header copy"
            );
            let end = end + SYSTEM_HEADER_LEN;
            if kind == SECTION_MAP_KIND {
                // (AutoCAD's rewritten section map in a file with gaps has other bytes here.)
                if gaps == 0 {
                    let fill = (sm.address + u64::from(sm.size)) as usize - end;
                    assert_eq!(bytes[end..end + fill], keystream[..fill], "{name}: fill");
                }
            } else {
                assert_eq!(
                    end as u64,
                    l.field64(0x34),
                    "{name}: second header position"
                );
            }
        }

        // Data pages: keystream after the payload (checked without gaps: AutoCAD's file with
        // gaps starts some fills further into the keystream), zero padding in a stored page.
        for d in &l.descs {
            for r in &d.pages {
                let page = l.page(r.number);
                let h = data_page_header(bytes, page.address);
                let end = (page.address + 0x20) as usize + h[2] as usize;
                let fill = (page.address + u64::from(page.size)) as usize - end;
                if gaps == 0 {
                    assert_eq!(
                        bytes[end..end + fill],
                        keystream[..fill],
                        "{name}: {}",
                        d.name
                    );
                }
                if d.compressed == 1 {
                    let logical = d
                        .size
                        .saturating_sub(r.start)
                        .min(u64::from(d.max_page_size));
                    let padding = &bytes[(page.address + 0x20 + logical) as usize..end];
                    assert!(
                        padding.iter().all(|&b| b == 0),
                        "{name}: {} padding",
                        d.name
                    );
                }
            }
        }
    }

    /// The R2018 files of the main corpus (`corpus/manifest.toml`: AutoCAD's
    /// own layouts; the extended corpus has other writers'), or a reported
    /// skip.
    fn corpus_r2018() -> Vec<(String, Vec<u8>)> {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("corpus");
        let manifest = std::fs::read_to_string(root.join("manifest.toml")).unwrap_or_default();
        let names: Vec<&str> = manifest
            .lines()
            .filter_map(|l| l.strip_prefix("name = \""))
            .filter_map(|l| l.strip_suffix('"'))
            .collect();
        let mut files: Vec<(String, Vec<u8>)> = names
            .iter()
            .filter_map(|n| {
                let file = format!("{n}.dwg");
                let bytes = std::fs::read(root.join("files").join(&file)).ok()?;
                bytes.starts_with(b"AC1032").then_some((file, bytes))
            })
            .collect();
        files.sort();
        if files.is_empty() {
            assert!(
                std::env::var("OPENDWG_REQUIRE_CORPUS").as_deref() != Ok("1"),
                "OPENDWG_REQUIRE_CORPUS=1 but the corpus has no AC1032 file"
            );
            eprintln!("SKIPPED: no AC1032 corpus files; run scripts/fetch-corpus.sh");
        }
        files
    }

    /// Our file against AutoCAD's, field by field. Allowed differences:
    /// - header data 0x2C (last page end), 0x34 (second header address), 0x54 (page map
    ///   address) and the CRC at 0x68: they follow from compressed sizes, which depend on the
    ///   compressor, and from the page map's extent (AutoCAD reserves about twice the map);
    /// - the data size and extent of compressed pages and of the system pages (same reason);
    /// - the order of page numbers within a section: AutoCAD numbers the two AcDsPrototype_1b
    ///   pages of acadsharp-sample-ac1032 out of file order (6, 5), we number in file order;
    /// - only for a file whose page map has gaps (acadsharp-aec-objects, an edited file): the
    ///   gap and page-numbering header fields 0x18, 0x1C, 0x20, 0x28, 0x3C, 0x40, 0x50, 0x5C,
    ///   0x60, 0x64 and the page numbers, since we write no gaps.
    ///
    /// Everything else must match: the whole plain header (section addresses included; see
    /// [`default_plain_header_matches_autocad`] for a header not copied from the input), every
    /// other header data field, the section map header, and per section in map order the name,
    /// size, max page size, unknown field, flags, section id, page count and page start
    /// offsets, stored pages' data sizes and extents, the unmasked page header words 0x00
    /// (type), 0x04 (section) and 0x10 (start), and the order of sections in the file. Both
    /// files must also pass [`check_invariants`].
    #[test]
    fn corpus_layout_matches_autocad() {
        for (name, bytes) in corpus_r2018() {
            let raw = read(&bytes).unwrap();
            let out = write_r2018(&raw).unwrap();
            assert!(read(&out).unwrap().warnings.is_empty(), "{name}");
            let (theirs, ours) = (layout(&bytes), layout(&out));
            check_invariants(&name, &bytes, &theirs);
            check_invariants(&name, &out, &ours);
            assert!(
                ours.entries.iter().all(|e| e.number > 0),
                "{name}: we write no gaps"
            );
            let gapless = theirs.entries.iter().all(|e| e.number > 0);

            assert_eq!(theirs.plain, ours.plain, "{name}: plain header");
            let mut fields = vec![0x00, 0x04, 0x08, 0x0C, 0x10, 0x14, 0x24, 0x44, 0x48, 0x4C];
            if gapless {
                fields.extend([0x18, 0x1C, 0x20, 0x28, 0x3C, 0x40, 0x50, 0x5C, 0x60, 0x64]);
            }
            for at in fields {
                assert_eq!(
                    theirs.field(at),
                    ours.field(at),
                    "{name}: header data {at:#x}"
                );
            }
            assert_eq!(theirs.section_map_head, ours.section_map_head, "{name}");
            assert_eq!(theirs.descs.len(), ours.descs.len(), "{name}");
            for (t, o) in theirs.descs.iter().zip(&ours.descs) {
                let n = &t.name;
                assert_eq!(t.name, o.name, "{name}: section map order");
                assert_eq!(t.size, o.size, "{name}: {n} size");
                assert_eq!(
                    t.max_page_size, o.max_page_size,
                    "{name}: {n} max page size"
                );
                assert_eq!(t.unknown, o.unknown, "{name}: {n} unknown field");
                assert_eq!(t.compressed, o.compressed, "{name}: {n} compressed");
                assert_eq!(t.section_id, o.section_id, "{name}: {n} section id");
                assert_eq!(t.encrypted, o.encrypted, "{name}: {n} encrypted");
                // Every page is stored, in order, by both.
                let page = u64::from(o.max_page_size);
                let want: Vec<u64> = (0..o.size.div_ceil(page)).map(|k| k * page).collect();
                for d in [t, o] {
                    let starts: Vec<u64> = d.pages.iter().map(|r| r.start).collect();
                    assert_eq!(starts, want, "{name}: {n} pages");
                }
                if gapless {
                    let numbers = |d: &SectionDesc| {
                        let mut v: Vec<u32> = d.pages.iter().map(|r| r.number).collect();
                        v.sort();
                        v
                    };
                    assert_eq!(numbers(t), numbers(o), "{name}: {n} page numbers");
                }
                for (tr, or) in t.pages.iter().zip(&o.pages) {
                    let (tp, op) = (theirs.page(tr.number), ours.page(or.number));
                    let th = data_page_header(&bytes, tp.address);
                    let oh = data_page_header(&out, op.address);
                    for w in [0, 1, 4, 5] {
                        assert_eq!(th[w], oh[w], "{name}: {n} page header word {w}");
                    }
                    if t.compressed == 1 {
                        assert_eq!(tr.data_size, or.data_size, "{name}: {n} data size");
                        assert_eq!(tp.size, op.size, "{name}: {n} page extent");
                    }
                }
            }
            assert_eq!(
                theirs.stream_order(),
                ours.stream_order(),
                "{name}: file order"
            );
        }
    }

    /// Without a kept header, the plain header is AutoCAD 2018's except for the undocumented
    /// longs at 0x38-0x44, which differ between AutoCAD builds: the default holds
    /// acadsharp-sample-ac1032's values (0x17B, 0x149, 0x149); acadsharp-aec-objects has
    /// 0xFF, 0xCD, 0xCD there. A header from another version is not used.
    #[test]
    fn default_plain_header_matches_autocad() {
        for (name, bytes) in corpus_r2018() {
            for kept in [None, Some(*b"AC1027")] {
                let mut raw = read(&bytes).unwrap();
                raw.file_header = kept.map(|magic| {
                    let mut h = [0x55; R2004_PLAIN_HEADER_LEN];
                    h[..6].copy_from_slice(&magic);
                    h
                });
                let out = write_r2018(&raw).unwrap();
                let mut theirs = bytes[..R2004_PLAIN_HEADER_LEN].to_vec();
                let mut ours = out[..R2004_PLAIN_HEADER_LEN].to_vec();
                if name.contains("aec-objects") {
                    assert_eq!(
                        theirs[0x38..0x44],
                        [0xFF, 0, 0, 0, 0xCD, 0, 0, 0, 0xCD, 0, 0, 0]
                    );
                    theirs[0x38..0x44].fill(0);
                    ours[0x38..0x44].fill(0);
                }
                assert_eq!(theirs, ours, "{name}");
            }
        }
    }

    fn sample() -> RawSections {
        RawSections {
            version: Version::R2018,
            maintenance: 0,
            codepage: 30,
            sections: BTreeMap::from([
                (SectionName::Header, vec![1; 0x40]),
                (SectionName::Classes, vec![2; 0x10]),
                (SectionName::Handles, vec![0, 2]),
                (SectionName::Objects, vec![4; 0x100]),
            ]),
            objects_base: 0,
            preview_offset: None,
            section_meta: BTreeMap::new(),
            file_header: None,
            warnings: Vec::new(),
        }
    }

    #[test]
    fn synthetic_file_reads_back_without_warnings() {
        let mut raw = sample();
        raw.sections.insert(SectionName::Template, vec![0; 6]);
        raw.sections.insert(SectionName::AppInfo, b"app".to_vec());
        let out = write_r2018(&raw).unwrap();
        let back = read(&out).unwrap();
        assert!(back.warnings.is_empty(), "{:?}", back.warnings);
        assert_eq!(back.sections.len(), raw.sections.len() + 1);
        for (name, data) in &raw.sections {
            assert_eq!(&back.sections[name], data, "{name:?}");
        }
        // An all-zero page is stored, not omitted.
        let l = layout(&out);
        let template = l.descs.iter().find(|d| d.name == "AcDb:Template").unwrap();
        assert_eq!(template.pages.len(), 1);
        check_invariants("synthetic", &out, &l);
        assert_eq!(back.file_header.unwrap(), {
            let mut h = DEFAULT_PLAIN_HEADER;
            h[..6].copy_from_slice(b"AC1032");
            h[0x13] = 30;
            h[0x28] = 0x80;
            h[OFF_APP_INFO..OFF_APP_INFO + 4]
                .copy_from_slice(&l.address_of("AcDb:AppInfo").to_le_bytes());
            h
        });
    }

    #[test]
    fn unknown_sections_keep_their_metadata() {
        let mut raw = sample();
        let name = SectionName::Other("AcDb:Mystery".into());
        let meta = SectionMeta {
            compressed: false,
            encrypted: 2,
            max_page_size: 0x100,
            unknown: 7,
        };
        raw.sections
            .insert(name.clone(), (0..0x250u32).map(|i| i as u8).collect());
        raw.section_meta.insert(name.clone(), meta);
        let back = read(&write_r2018(&raw).unwrap()).unwrap();
        assert!(back.warnings.is_empty(), "{:?}", back.warnings);
        assert_eq!(back.sections[&name], raw.sections[&name]);
        assert_eq!(back.section_meta[&name], meta);

        // Without metadata: compressed 0x7400 pages, unknown field 1, not encrypted.
        let mut raw = sample();
        raw.sections.insert(name.clone(), vec![5; 3]);
        let back = read(&write_r2018(&raw).unwrap()).unwrap();
        assert_eq!(
            back.section_meta[&name],
            SectionMeta {
                compressed: true,
                encrypted: 0,
                max_page_size: 0x7400,
                unknown: 1,
            }
        );
    }

    #[test]
    fn unwritable_inputs_error() {
        let mut raw = sample();
        raw.version = Version::R2013;
        assert!(matches!(
            write_r2018(&raw),
            Err(Error::Encode(m)) if m.contains("AC1027")
        ));

        let mut raw = sample();
        raw.sections.remove(&SectionName::Handles);
        assert!(matches!(write_r2018(&raw), Err(Error::Encode(m)) if m.contains("Handles")));

        for bad in ["AcDb:Header", "AcDb:Objects", &"x".repeat(64), "a\0b"] {
            let mut raw = sample();
            raw.sections.insert(SectionName::Other(bad.into()), vec![1]);
            assert!(
                matches!(write_r2018(&raw), Err(Error::Encode(_))),
                "{bad:?}"
            );
        }

        let mut raw = sample();
        let name = SectionName::Other("AcDb:Locked".into());
        raw.sections.insert(name.clone(), vec![1]);
        raw.section_meta.insert(
            name,
            SectionMeta {
                compressed: true,
                encrypted: 1,
                max_page_size: 0x7400,
                unknown: 1,
            },
        );
        assert!(matches!(write_r2018(&raw), Err(Error::Encode(m)) if m.contains("encrypted")));
    }

    #[test]
    fn invalid_metadata_errors() {
        for (compressed, max_page_size) in [(true, 0), (false, 0), (true, 0x7401)] {
            let mut raw = sample();
            let name = SectionName::Other("AcDb:Odd".into());
            raw.sections.insert(name.clone(), vec![1; 0x10]);
            let meta = SectionMeta {
                compressed,
                encrypted: 0,
                max_page_size,
                unknown: 1,
            };
            raw.section_meta.insert(name, meta);
            assert!(
                matches!(write_r2018(&raw), Err(Error::Encode(m)) if m.contains("page size")),
                "{meta:?}"
            );
        }
    }

    #[test]
    fn huge_stored_page_size_is_cut_to_the_data() {
        let mut raw = sample();
        let name = SectionName::Other("AcDb:Small".into());
        raw.sections.insert(name.clone(), vec![1; 0x10]);
        let meta = SectionMeta {
            compressed: false,
            encrypted: 0,
            max_page_size: 0x1000_0000,
            unknown: 1,
        };
        raw.section_meta.insert(name.clone(), meta);
        let out = write_r2018(&raw).unwrap();
        assert!(out.len() < 0x10_000);
        let back = read(&out).unwrap();
        assert_eq!(back.section_meta[&name].max_page_size, 0x80);
        assert_eq!(back.sections[&name], raw.sections[&name]);
    }

    #[test]
    fn uncharted_known_sections_prefer_read_metadata() {
        let meta = SectionMeta {
            compressed: false,
            encrypted: 2,
            max_page_size: 0x100,
            unknown: 5,
        };
        for name in [
            SectionName::Signature,
            SectionName::Security,
            SectionName::VbaProject,
            SectionName::FileDepList,
        ] {
            let mut raw = sample();
            raw.sections.insert(name.clone(), vec![6; 0x180]);
            let ruled = read(&write_r2018(&raw).unwrap()).unwrap().section_meta[&name];
            raw.section_meta.insert(name.clone(), meta);
            let back = read(&write_r2018(&raw).unwrap()).unwrap();
            assert_eq!(back.section_meta[&name], meta, "{name:?}");
            assert_eq!(back.sections[&name], raw.sections[&name], "{name:?}");
            assert_ne!(ruled, meta, "{name:?}");
        }
        // Known sections with corpus evidence keep AutoCAD's rule.
        let mut raw = sample();
        raw.section_meta.insert(SectionName::Header, meta);
        let back = read(&write_r2018(&raw).unwrap()).unwrap();
        assert!(back.section_meta[&SectionName::Header].compressed);
    }

    #[test]
    fn security_flags_are_cleared() {
        let mut raw = sample();
        let mut h = DEFAULT_PLAIN_HEADER;
        h[..6].copy_from_slice(b"AC1032");
        h[SECURITY_FIELDS].fill(0xFF);
        h[0x38] = 0x42;
        raw.file_header = Some(h);
        let out = write_r2018(&raw).unwrap();
        assert!(out[SECURITY_FIELDS].iter().all(|&b| b == 0));
        assert_eq!(out[0x38], 0x42, "other kept bytes stay");
    }

    #[test]
    fn large_section_page_count() {
        let mut raw = sample();
        let mut s = 1u32;
        let objects: Vec<u8> = (0..5 << 20)
            .map(|_| {
                s = s.wrapping_mul(0x343FD).wrapping_add(0x269EC3);
                (s >> 16) as u8
            })
            .collect();
        raw.sections.insert(SectionName::Objects, objects.clone());
        let out = write_r2018(&raw).unwrap();
        let l = layout(&out);
        let d = l
            .descs
            .iter()
            .find(|d| d.name == "AcDb:AcDbObjects")
            .unwrap();
        assert_eq!(d.pages.len(), objects.len().div_ceil(0x7400));
        assert_eq!(read(&out).unwrap().sections[&SectionName::Objects], objects);
    }

    #[test]
    fn oversized_pages_error() {
        // Sizes beyond the 32-bit page fields are refused before anything is allocated.
        let huge = u64::from(u32::MAX) + 1;
        for (len, rule) in [
            (huge, PageRule::RoundUp(0x80)),
            (MAX_PAGE_DATA + 1, PageRule::RoundUp(0x400)),
            (huge, PageRule::PlusHeader),
            (u64::MAX - 0x10, PageRule::PlusHeader),
            (0, PageRule::Fixed(u32::MAX)),
        ] {
            assert!(
                matches!(page_size("s", len, rule), Err(Error::Encode(_))),
                "{len:#x} {rule:?}"
            );
        }
        assert_eq!(page_size("s", 0, PageRule::RoundUp(0x80)).unwrap(), 0x80);
        assert_eq!(
            page_size("s", 0x60c, PageRule::RoundUp(0x400)).unwrap(),
            0x800
        );
        assert_eq!(
            page_size("s", 0x5c6, PageRule::RoundUp(0x80)).unwrap(),
            0x600
        );
        assert_eq!(page_size("s", 0x81, PageRule::PlusHeader).unwrap(), 0x120);
        assert!(matches!(
            r2018_preview_address(Some(usize::MAX)),
            Err(Error::Encode(_))
        ));
    }

    #[test]
    fn preview_address_is_predicted_and_checked() {
        for summary in [
            None,
            Some(0),
            Some(0x4c),
            Some(0x80),
            Some(0x81),
            Some(0x1234),
        ] {
            let mut raw = sample();
            if let Some(n) = summary {
                raw.sections.insert(SectionName::SummaryInfo, vec![9; n]);
            }
            raw.sections.insert(SectionName::Preview, vec![8; 0x300]);
            let want = r2018_preview_address(summary).unwrap();
            raw.preview_offset = Some(want);
            let out = write_r2018(&raw).unwrap();
            let l = layout(&out);
            assert_eq!(u64::from(l.address_of("AcDb:Preview")), want, "{summary:?}");
            assert_eq!(read(&out).unwrap().preview_offset, Some(want));

            let mut no_preview = raw.clone();
            no_preview.sections.remove(&SectionName::Preview);
            assert!(
                matches!(write_r2018(&no_preview), Err(Error::Encode(m)) if m.contains("no preview")),
                "{summary:?}"
            );

            raw.preview_offset = Some(want + 0x20);
            assert!(
                matches!(write_r2018(&raw), Err(Error::Encode(m)) if m.contains("preview")),
                "{summary:?}"
            );
        }
    }

    #[test]
    fn corpus_preview_address_is_predicted() {
        for (name, bytes) in corpus_r2018() {
            let raw = read(&bytes).unwrap();
            let summary = raw.sections.get(&SectionName::SummaryInfo).map(Vec::len);
            assert_eq!(
                raw.preview_offset,
                Some(r2018_preview_address(summary).unwrap()),
                "{name}"
            );
        }
    }

    /// A prefix of the output that cuts into the data fails; one that holds every section
    /// and lacks only (part of) the trailing header copy reads, with a warning.
    #[test]
    fn every_prefix_of_output_errors_or_warns() {
        let out = write_r2018(&sample()).unwrap();
        let read_whole = read(&out).unwrap();
        for n in 0..out.len() {
            if let Ok(raw) = read(&out[..n]) {
                assert_eq!(raw.sections, read_whole.sections, "prefix {n:#x}");
                assert!(
                    raw.warnings
                        .iter()
                        .any(|w| w.contains("file header missing")),
                    "prefix {n:#x}: {:?}",
                    raw.warnings
                );
            }
        }
        // 0x100 bytes short, data is missing.
        assert!(read(&out[..out.len() - 0x100]).is_err());
    }
}
