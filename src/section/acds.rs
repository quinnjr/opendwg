//! The data storage section, `AcDb:AcDsPrototype_1b` (ODS 24): R2013+
//! keeps the modeler (SAB) data of REGION, 3DSOLID and BODY entities here,
//! and AutoCAD the block thumbnails, as data records keyed by handle.
//!
//! The section is a byte stream: a 0x80-byte file header, then file
//! segments, each a 0x30-byte header and data padded with 0x70 to a multiple
//! of 0x80 bytes. The segment index segment ("segidx") gives every
//! segment's offset and size by number; the header names the schema index
//! ("schidx"), data index ("datidx") and search segments by number. The
//! schemas live in schema data segments ("schdat"), the records in data
//! segments ("_data_") and, when large, blob pages ("blob01").
//!
//! [`DataStorage::parse`] reads the schemas and the live records (those the
//! data index lists). [`DataStorage::encode`] writes them in a fresh layout,
//! so a parsed section encodes to one that parses back to the same value,
//! not to the same bytes: the drawing writer copies the bytes read while the
//! value is unchanged (see `drawing::write`).
//!
//! Where the corpus (every R2013 and R2018 file, all written by AutoCAD)
//! differs from ODS or goes beyond it:
//! - The file header has two more fields after the file size (a 0 that may
//!   be its high half): the number of a free space segment ("freesp") and a
//!   count, both 0 without one. AutoCAD writes 8 where ODS has "Unknown 1
//!   (always 2?)".
//! - Offsets inside a segment (the schema index's schema and property
//!   entries, the data index's record headers) count from the end of the
//!   segment header; ODS says from the segment start. The alignment offsets
//!   of the segment header (x 16) count from the segment start, as ODS says.
//! - The schema index segment, after ODS's two tables, holds the schema
//!   names (a `UInt32` count, then zero-terminated strings) at its system
//!   data alignment offset, in table order; a schema data segment holds its
//!   property names the same way (ODS 24.2.2.6 places them after the
//!   schemas, with no position).
//! - ODS's "unknown properties" of the schema index are the schemas (index,
//!   segment, offset); its "property entries" point to the `UInt32` pairs
//!   ODS calls schema unknown properties ([`UnknownProperty`]). Padding
//!   before an aligned table is 0x73 bytes, before the data records 0x62.
//! - AutoCAD also writes a "prvsav" segment (a copy of the previous save's
//!   file header, named by ODS's "previous save index") and "freesp"
//!   segments, and leaves stale segments and records of earlier saves in
//!   place. [`DataStorage::encode`] writes neither: previous save and free
//!   space are 0, as in a fresh file. Unconfirmed against AutoCAD: whether
//!   it reads that layout.
//! - The data index lists records by schema, then handle; one handle may
//!   have several records (AecObjects.dwg has two thumbnails for a block).
//!   The search segment, which ODS describes, is derived from the records
//!   ([`DataStorage::search`] gives what is written, and matches every
//!   corpus file's).
//!
//! Unconfirmed by the corpus, which has none: blob records (ODS 24.2.2.3.1,
//! records over 0x40000 bytes) and stub data index entries (segment 0,
//! skipped as ODS says). Blob records follow ODS; the reference record's
//! "record size" is taken as the size of its fields after the marker.

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::error::{Error, Result};
use crate::handle::Handle;

/// The schema of ACIS (SAB) records, keyed by the entity's handle.
pub const ACIS_SCHEMA: &str = "AcDb3DSolid_ASM_Data";

/// The file header signature, "jard".
const SIGNATURE: u32 = 0x6472_616A;
/// The file header's second field (ODS: file header size), as AutoCAD
/// writes it.
const HEADER_SIZE_FIELD: u32 = 0x0001_0080;
/// ODS "Unknown 1": AutoCAD writes 8.
const HEADER_UNKNOWN_1: u32 = 8;
const HEADER_VERSION: u32 = 2;
const FILE_HEADER_LEN: usize = 0x80;
const SEGMENT_HEADER_LEN: usize = 0x30;
const SEGMENT_SIGNATURE: u16 = 0xD5AC;
/// Segments are padded to a multiple of this (AutoCAD; ODS allows 0x40).
const SEGMENT_ALIGN: usize = 0x80;
const SEGMENT_PAD: u8 = 0x70;
const TABLE_PAD: u8 = 0x73;
const DATA_PAD: u8 = 0x62;
/// The 8 alignment bytes ending a segment header.
const ALIGNMENT_BYTES: [u8; 8] = [0x55; 8];
/// A record's data size field marking a blob reference.
const BLOB_MARKER: u32 = 0xBB10_6BB1;
/// The size of a data record header, its first field.
const RECORD_HEADER_LEN: u32 = 0x14;
/// Records larger than this are stored as blobs (ODS 24.2.2.3).
const INLINE_MAX: usize = 0x4_0000;
/// The most data a blob page holds (ODS 24.2.2.3).
const BLOB_PAGE_MAX: usize = 0xF_FFB0;
/// The 64-bit value between the schema index's two tables (ODS).
const SCHEMA_INDEX_MAGIC: u64 = 0x0A_F10C;
/// Value sizes by property type (ODS 24.2.2.6.1.1); type 0xE stores its
/// own.
const TYPE_SIZES: [usize; 16] = [0, 0, 2, 1, 2, 4, 8, 1, 2, 4, 8, 4, 8, 0, 0, 0];
const CUSTOM_TYPE: u32 = 0xE;

/// The data storage section (see the module documentation).
#[derive(Debug, Clone, PartialEq, Eq, Default, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct DataStorage {
    /// The data storage revision, which AutoCAD raises on every save.
    pub revision: u32,
    /// The schemas, in schema index order.
    pub schemas: Vec<Schema>,
    /// ODS's schema unknown properties, in schema index order.
    pub unknown_properties: Vec<UnknownProperty>,
    /// The records, in data index order: by schema, then handle, as
    /// AutoCAD writes them ([`DataStorage::parse`] and deserializing sort
    /// records stored out of that order, stably). [`DataStorage::acis_data`]
    /// finds records by binary search in this order: after changing the
    /// records by hand, keep it, or call [`DataStorage::sort_records`].
    #[cfg_attr(feature = "serde", serde(deserialize_with = "sorted_records"))]
    pub records: Vec<DataRecord>,
}

/// Deserialize records into data index order (see
/// [`DataStorage::records`]).
#[cfg(feature = "serde")]
fn sorted_records<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> std::result::Result<Vec<DataRecord>, D::Error> {
    let mut records: Vec<DataRecord> = serde::Deserialize::deserialize(d)?;
    records.sort_by_key(DataRecord::order);
    Ok(records)
}

#[cfg(test)]
thread_local! {
    /// The records [`DataStorage::acis_data`] compared, for work tests.
    static ACIS_PROBES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// A schema (ODS 24.2.2.6.1).
#[derive(Debug, Clone, PartialEq, Eq, Default, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct Schema {
    /// The schema's number, which records name it by.
    pub index: u32,
    pub name: String,
    pub indexes: Vec<u64>,
    pub properties: Vec<SchemaProperty>,
}

/// A schema property (ODS 24.2.2.6.1.1).
#[derive(Debug, Clone, PartialEq, Eq, Default, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct SchemaProperty {
    /// 1 and 8 stand alone and add [`SchemaProperty::unknown`]; 2 means no
    /// type.
    pub flags: u32,
    pub name: String,
    /// The type (0-15), stored unless flag 2 is set (then 0).
    pub type_code: u32,
    /// Type 0xE: the size of each value.
    pub custom_size: u32,
    /// Flags 1 or 8: the `UInt32` that follows.
    pub unknown: u32,
    pub value_count: u16,
    /// The values: `value_count` of [`SchemaProperty::value_size`] bytes,
    /// one after another (none when the size is 0).
    pub values: Vec<u8>,
}

impl SchemaProperty {
    fn has_type(&self) -> bool {
        self.flags & 2 == 0
    }

    /// The size of each value.
    pub fn value_size(&self) -> Result<usize> {
        if !self.has_type() {
            return Ok(0);
        }
        if self.type_code == CUSTOM_TYPE {
            return usize::try_from(self.custom_size).map_err(|_| corrupt("value size"));
        }
        usize::try_from(self.type_code)
            .ok()
            .and_then(|t| TYPE_SIZES.get(t).copied())
            .ok_or_else(|| corrupt(format!("property type {}", self.type_code)))
    }
}

/// An entry ODS calls a schema unknown property: an index and two
/// `UInt32`s.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct UnknownProperty {
    pub index: u32,
    pub data_size: u32,
    pub flags: u32,
}

/// A data record: the data of one object under one schema.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct DataRecord {
    pub handle: Handle,
    /// The [`Schema::index`] of the record's schema.
    pub schema: u32,
    pub data: Vec<u8>,
}

impl DataRecord {
    /// The record's key in data index order: schema, then handle.
    fn order(&self) -> (u32, Handle) {
        (self.schema, self.handle)
    }
}

/// One schema's search data (ODS 24.2.2.7.1).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SearchSchema {
    /// The [`Schema::index`].
    pub schema: u32,
    /// The data index positions of the schema's records.
    pub sorted: Vec<u64>,
    /// Sets of (handle, data index positions of its records).
    pub ids: Vec<Vec<(Handle, Vec<u64>)>>,
}

fn corrupt(m: impl std::fmt::Display) -> Error {
    Error::Corrupt(format!("AcDs: {m}"))
}

/// A value that cannot be encoded (see [`Error::Encode`]).
fn encode_err(m: impl std::fmt::Display) -> Error {
    Error::Encode(format!("AcDs: {m}"))
}

fn truncated() -> Error {
    Error::Truncated {
        context: "data storage section",
    }
}

fn usize_of(v: impl TryInto<usize>) -> Result<usize> {
    v.try_into().map_err(|_| truncated())
}

/// Reads little-endian values from `bytes[pos..end]`.
struct At<'a> {
    bytes: &'a [u8],
    pos: usize,
    end: usize,
}

impl<'a> At<'a> {
    fn new(bytes: &'a [u8], pos: usize, end: usize) -> Result<Self> {
        if pos > end || end > bytes.len() {
            return Err(truncated());
        }
        Ok(At { bytes, pos, end })
    }

    fn left(&self) -> usize {
        self.end - self.pos
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self
            .pos
            .checked_add(n)
            .filter(|&e| e <= self.end)
            .ok_or_else(truncated)?;
        let s = &self.bytes[self.pos..end];
        self.pos = end;
        Ok(s)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        let mut a = [0; N];
        a.copy_from_slice(self.take(N)?);
        Ok(a)
    }

    fn u16(&mut self) -> Result<u16> {
        self.array().map(u16::from_le_bytes)
    }

    fn u32(&mut self) -> Result<u32> {
        self.array().map(u32::from_le_bytes)
    }

    fn u64(&mut self) -> Result<u64> {
        self.array().map(u64::from_le_bytes)
    }

    /// A count of items of `size` bytes each, checked against the bytes
    /// left.
    fn count(&mut self, n: u64, size: usize) -> Result<usize> {
        let n = usize::try_from(n).map_err(|_| truncated())?;
        if n.checked_mul(size).is_none_or(|b| b > self.left()) {
            return Err(corrupt(format!("count {n} exceeds the segment")));
        }
        Ok(n)
    }

    fn count_u16(&mut self, size: usize) -> Result<usize> {
        let n = self.u16()?;
        self.count(u64::from(n), size)
    }

    fn count_u32(&mut self, size: usize) -> Result<usize> {
        let n = self.u32()?;
        self.count(u64::from(n), size)
    }

    fn count_u64(&mut self, size: usize) -> Result<usize> {
        let n = self.u64()?;
        self.count(n, size)
    }

    /// A zero-terminated string; bytes are taken as Latin-1, so every byte
    /// string survives.
    fn cstr(&mut self) -> Result<String> {
        let rest = &self.bytes[self.pos..self.end];
        let n = rest.iter().position(|&b| b == 0).ok_or_else(truncated)?;
        let s = rest[..n].iter().map(|&b| char::from(b)).collect();
        self.pos += n + 1;
        Ok(s)
    }
}

/// A segment as found through the segment index.
#[derive(Debug, Clone, Copy)]
struct Segment {
    /// Its offset in the section.
    start: usize,
    /// Its end (start + size).
    end: usize,
    name: [u8; 6],
    sys_align: usize,
    obj_align: usize,
}

impl Segment {
    fn body(&self) -> usize {
        self.start + SEGMENT_HEADER_LEN
    }

    /// A reader at `offset` past the segment header.
    fn at<'a>(&self, bytes: &'a [u8], offset: u64) -> Result<At<'a>> {
        let pos = usize_of(offset)?
            .checked_add(self.body())
            .ok_or_else(truncated)?;
        At::new(bytes, pos, self.end)
    }

    /// A reader at `align` x 16 from the segment start.
    fn aligned<'a>(&self, bytes: &'a [u8], align: usize) -> Result<At<'a>> {
        let pos = align
            .checked_mul(16)
            .and_then(|o| o.checked_add(self.start))
            .ok_or_else(truncated)?;
        if pos < self.body() {
            return Err(corrupt("alignment offset inside the segment header"));
        }
        At::new(bytes, pos, self.end)
    }

    fn expect(&self, name: &[u8; 6]) -> Result<()> {
        if &self.name == name {
            Ok(())
        } else {
            Err(corrupt(format!(
                "segment {:?} where {:?} was expected",
                String::from_utf8_lossy(&self.name),
                String::from_utf8_lossy(name)
            )))
        }
    }
}

/// The section's segments by number.
struct Segments<'a> {
    bytes: &'a [u8],
    /// (offset, size) by segment number.
    entries: Vec<(u64, u32)>,
    /// The bytes the parser may still decode into the value: twice the
    /// section, plus 4 KiB. Schemas, names and records are copied out of the section;
    /// a hostile index pointing many entries at one large item would
    /// otherwise copy it again for each.
    budget: std::cell::Cell<usize>,
}

impl Segments<'_> {
    /// Charge `n` decoded bytes against the budget.
    fn spend(&self, n: usize) -> Result<()> {
        let left = self
            .budget
            .get()
            .checked_sub(n)
            .ok_or_else(|| corrupt("the indexes repeat more data than the section holds"))?;
        self.budget.set(left);
        Ok(())
    }

    fn get(&self, index: u32) -> Result<Segment> {
        let &(offset, size) = usize::try_from(index)
            .ok()
            .and_then(|i| self.entries.get(i))
            .ok_or_else(|| corrupt(format!("no segment {index}")))?;
        segment_at(self.bytes, offset, u64::from(size))
    }
}

/// The segment whose header is at `offset`, `size` bytes long.
fn segment_at(bytes: &[u8], offset: u64, size: u64) -> Result<Segment> {
    let start = usize_of(offset)?;
    let end = start
        .checked_add(usize_of(size)?)
        .filter(|&e| e <= bytes.len())
        .ok_or_else(truncated)?;
    if end - start < SEGMENT_HEADER_LEN {
        return Err(corrupt("segment smaller than its header"));
    }
    let mut r = At::new(bytes, start, end)?;
    if r.u16()? != SEGMENT_SIGNATURE {
        return Err(corrupt(format!("no segment signature at {start:#x}")));
    }
    let name = r.array()?;
    let _index = r.u32()?;
    let _unknown1 = r.u32()?;
    let _size = r.u32()?;
    let _unknown2 = r.u32()?;
    let _revision = r.u32()?;
    let _unknown3 = r.u32()?;
    let sys_align = usize_of(r.u32()?)?;
    let obj_align = usize_of(r.u32()?)?;
    Ok(Segment {
        start,
        end,
        name,
        sys_align,
        obj_align,
    })
}

/// The file header fields the parser uses.
struct FileHeader {
    revision: u32,
    segidx_offset: u32,
    segidx_count: u32,
    schidx: u32,
    datidx: u32,
    search: u32,
}

fn file_header(bytes: &[u8]) -> Result<FileHeader> {
    let mut r = At::new(bytes, 0, bytes.len().min(FILE_HEADER_LEN))?;
    if r.left() < FILE_HEADER_LEN {
        return Err(truncated());
    }
    if r.u32()? != SIGNATURE {
        return Err(corrupt("bad file signature"));
    }
    let _size = r.u32()?;
    let _unknown1 = r.u32()?;
    let _version = r.u32()?;
    let _unknown2 = r.u32()?;
    let revision = r.u32()?;
    let segidx_offset = r.u32()?;
    let _segidx_unknown = r.u32()?;
    let segidx_count = r.u32()?;
    Ok(FileHeader {
        revision,
        segidx_offset,
        segidx_count,
        schidx: r.u32()?,
        datidx: r.u32()?,
        search: r.u32()?,
    })
}

/// The segment index: (offset, size) of every segment.
fn segment_index<'a>(bytes: &'a [u8], h: &FileHeader) -> Result<Segments<'a>> {
    // The segment index's own size is in its entry, which is in it: take
    // the rest of the section and check the header's size field.
    let offset = u64::from(h.segidx_offset);
    let rest = (bytes.len() as u64).saturating_sub(offset);
    let seg = segment_at(bytes, offset, rest)?;
    seg.expect(b"segidx")?;
    let size = At::new(bytes, seg.start + 16, seg.end)?.u32()?;
    let seg = segment_at(bytes, offset, u64::from(size))?;
    let mut r = seg.at(bytes, 0)?;
    let n = r.count(u64::from(h.segidx_count), 12)?;
    let mut entries = Vec::with_capacity(n);
    for _ in 0..n {
        entries.push((r.u64()?, r.u32()?));
    }
    Ok(Segments {
        bytes,
        entries,
        budget: std::cell::Cell::new(bytes.len().saturating_mul(2).saturating_add(4096)),
    })
}

/// The zero-terminated names of a schema segment, at its system data
/// alignment offset.
///
/// Each name is charged to the budget (its bytes and a per-entry overhead)
/// before it is copied: segment numbers may alias one segment, which would
/// otherwise copy its table once per number.
fn name_table(segs: &Segments, seg: &Segment) -> Result<Vec<String>> {
    let mut r = seg.aligned(segs.bytes, seg.sys_align)?;
    let n = r.count_u32(1)?;
    let mut names = Vec::with_capacity(n);
    for _ in 0..n {
        let rest = &r.bytes[r.pos..r.end];
        let len = rest.iter().position(|&b| b == 0).unwrap_or(rest.len());
        segs.spend(len.saturating_add(NAME_OVERHEAD))?;
        names.push(r.cstr()?);
    }
    Ok(names)
}

/// What each name costs the budget besides its bytes.
const NAME_OVERHEAD: usize = 8;

/// A schema property; its name, values and fixed fields are charged to the
/// budget before they are copied.
fn parse_property(segs: &Segments, r: &mut At, names: &[String]) -> Result<SchemaProperty> {
    let flags = r.u32()?;
    let name_index = r.u32()?;
    let name = usize::try_from(name_index)
        .ok()
        .and_then(|i| names.get(i))
        .ok_or_else(|| corrupt(format!("property name {name_index}")))?;
    segs.spend(name.len().saturating_add(10))?;
    let name = name.clone();
    let mut p = SchemaProperty {
        flags,
        name,
        ..SchemaProperty::default()
    };
    if p.has_type() {
        p.type_code = r.u32()?;
        if p.type_code == CUSTOM_TYPE {
            p.custom_size = r.u32()?;
        }
    }
    if flags == 1 || flags == 8 {
        p.unknown = r.u32()?;
    }
    p.value_count = r.u16()?;
    let size = p.value_size()?;
    if size != 0 {
        let n = r.count(u64::from(p.value_count), size)?;
        segs.spend(n * size)?;
        p.values = r.take(n * size)?.to_vec();
    }
    Ok(p)
}

fn parse_schema(
    segs: &Segments,
    r: &mut At,
    index: u32,
    name: String,
    names: &[String],
) -> Result<Schema> {
    let n = r.count_u16(8)?;
    segs.spend(n * 8)?;
    let indexes = (0..n).map(|_| r.u64()).collect::<Result<_>>()?;
    // The smallest property: flags, name, value count.
    let n = r.count_u16(10)?;
    let properties = (0..n)
        .map(|_| parse_property(segs, r, names))
        .collect::<Result<_>>()?;
    Ok(Schema {
        index,
        name,
        indexes,
        properties,
    })
}

/// The schemas and unknown properties, from the schema index segment and
/// the schema data segments it points to.
fn parse_schemas(segs: &Segments, schidx: u32) -> Result<(Vec<Schema>, Vec<UnknownProperty>)> {
    let bytes = segs.bytes;
    let seg = segs.get(schidx)?;
    seg.expect(b"schidx")?;
    let schema_names = name_table(segs, &seg)?;
    let mut r = seg.at(bytes, 0)?;
    let n = r.count_u32(12)?;
    let _unknown = r.u32()?;
    let mut entries = Vec::with_capacity(n);
    for _ in 0..n {
        entries.push((r.u32()?, r.u32()?, r.u32()?));
    }
    let _magic = r.u64()?;
    let m = r.count_u32(12)?;
    let _unknown = r.u32()?;
    let mut props = Vec::with_capacity(m);
    for _ in 0..m {
        props.push((r.u32()?, r.u32()?, r.u32()?));
    }
    // Each schema data segment and its property names, read once per
    // segment extent (several numbers may name one segment).
    let mut data_segs: HashMap<(usize, usize), (Segment, Vec<String>)> = HashMap::new();
    let mut extent_of: HashMap<u32, (usize, usize)> = HashMap::new();
    for &(_, seg_index, _) in &entries {
        if extent_of.contains_key(&seg_index) {
            continue;
        }
        let s = segs.get(seg_index)?;
        s.expect(b"schdat")?;
        let key = (s.start, s.end);
        extent_of.insert(seg_index, key);
        if let std::collections::hash_map::Entry::Vacant(e) = data_segs.entry(key) {
            e.insert((s, name_table(segs, &s)?));
        }
    }
    let mut schemas = Vec::with_capacity(n);
    for (k, (index, seg_index, offset)) in entries.into_iter().enumerate() {
        let name = schema_names
            .get(k)
            .ok_or_else(|| corrupt(format!("schema {index} has no name")))?
            .clone();
        let (s, names) = &data_segs[&extent_of[&seg_index]];
        let mut r = s.at(bytes, u64::from(offset))?;
        schemas.push(parse_schema(segs, &mut r, index, name, names)?);
    }
    let mut unknown = Vec::with_capacity(m);
    for (seg_index, offset, index) in props {
        let s = match extent_of.get(&seg_index) {
            Some(key) => data_segs[key].0,
            None => {
                let s = segs.get(seg_index)?;
                s.expect(b"schdat")?;
                s
            }
        };
        let mut r = s.at(bytes, u64::from(offset))?;
        unknown.push(UnknownProperty {
            index,
            data_size: r.u32()?,
            flags: r.u32()?,
        });
    }
    Ok((schemas, unknown))
}

/// A blob reference record's data (ODS 24.2.2.3.1), `r` past the marker.
fn parse_blob(segs: &Segments, r: &mut At<'_>) -> Result<Vec<u8>> {
    let total = r.u64()?;
    let pages = r.u32()?;
    let _record_size = r.u32()?;
    let _page_size = r.u32()?;
    let _last_page_size = r.u32()?;
    let _unknown1 = r.u32()?;
    let _unknown2 = r.u32()?;
    let pages = r.count(u64::from(pages), 8)?;
    // The pages hold the data, so it cannot exceed the section.
    if total > segs.bytes.len() as u64 {
        return Err(corrupt(format!(
            "blob of {total} bytes exceeds the section"
        )));
    }
    segs.spend(usize_of(total)?)?;
    let mut data = Vec::with_capacity(usize_of(total)?);
    for _ in 0..pages {
        let seg = segs.get(r.u32()?)?;
        let _size = r.u32()?;
        seg.expect(b"blob01")?;
        let mut p = seg.at(segs.bytes, 0)?;
        let _total = p.u64()?;
        let start = p.u64()?;
        let _page_index = p.u32()?;
        let _page_count = p.u32()?;
        let size = usize_of(p.u64()?)?;
        if start != data.len() as u64 {
            return Err(corrupt("blob pages out of order"));
        }
        let page = p.take(size)?;
        if (data.len() + page.len()) as u64 > total {
            return Err(corrupt("blob pages exceed its size"));
        }
        data.extend_from_slice(page);
    }
    if data.len() as u64 != total {
        return Err(corrupt("blob pages short of its size"));
    }
    Ok(data)
}

/// The records the data index lists, in its order.
fn parse_records(segs: &Segments, datidx: u32) -> Result<Vec<DataRecord>> {
    let bytes = segs.bytes;
    let seg = segs.get(datidx)?;
    seg.expect(b"datidx")?;
    let mut r = seg.at(bytes, 0)?;
    let n = r.count_u32(12)?;
    let _unknown = r.u32()?;
    let mut entries = Vec::with_capacity(n);
    for _ in 0..n {
        entries.push((r.u32()?, r.u32()?, r.u32()?));
    }
    let mut records = Vec::with_capacity(n);
    for (seg_index, offset, schema) in entries {
        if seg_index == 0 {
            continue; // A stub entry (ODS).
        }
        let data_seg = segs.get(seg_index)?;
        data_seg.expect(b"_data_")?;
        let mut h = data_seg.at(bytes, u64::from(offset))?;
        let _entry_size = h.u32()?;
        let _unknown = h.u32()?;
        let handle = Handle(h.u64()?);
        let local = h.u32()?;
        let base = data_seg.aligned(bytes, data_seg.obj_align)?;
        let mut d = At::new(
            bytes,
            base.pos
                .checked_add(usize_of(local)?)
                .ok_or_else(truncated)?,
            data_seg.end,
        )?;
        let size = d.u32()?;
        let data = if size == BLOB_MARKER {
            parse_blob(segs, &mut d)?
        } else {
            let data = d.take(usize_of(size)?)?;
            segs.spend(data.len())?;
            data.to_vec()
        };
        records.push(DataRecord {
            handle,
            schema,
            data,
        });
    }
    Ok(records)
}

fn parse_search(segs: &Segments, search: u32) -> Result<Vec<SearchSchema>> {
    let seg = segs.get(search)?;
    seg.expect(b"search")?;
    let mut r = seg.at(segs.bytes, 0)?;
    // The smallest schema search data: name index, sorted count, set count.
    let n = r.count_u32(16)?;
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        let schema = r.u32()?;
        let k = r.count_u64(8)?;
        let sorted = (0..k).map(|_| r.u64()).collect::<Result<_>>()?;
        let sets = r.count_u32(4)?;
        let mut ids = Vec::with_capacity(sets);
        if sets > 0 {
            let _unknown = r.u32()?;
        }
        for _ in 0..sets {
            let items = r.count_u32(16)?;
            let mut set = Vec::with_capacity(items);
            for _ in 0..items {
                let handle = Handle(r.u64()?);
                let k = r.count_u64(8)?;
                set.push((handle, (0..k).map(|_| r.u64()).collect::<Result<_>>()?));
            }
            ids.push(set);
        }
        out.push(SearchSchema {
            schema,
            sorted,
            ids,
        });
    }
    Ok(out)
}

impl DataStorage {
    /// Read the section.
    pub fn parse(bytes: &[u8]) -> Result<DataStorage> {
        let h = file_header(bytes)?;
        let segs = segment_index(bytes, &h)?;
        let (schemas, unknown_properties) = parse_schemas(&segs, h.schidx)?;
        let records = parse_records(&segs, h.datidx)?;
        let mut s = DataStorage {
            revision: h.revision,
            schemas,
            unknown_properties,
            records,
        };
        s.sort_records();
        Ok(s)
    }

    /// The search segment of the section `bytes` as stored, for inspection:
    /// [`DataStorage::encode`] writes [`DataStorage::search`] instead. Its
    /// positions are those of the records as stored: when the file stored
    /// them out of data index order they are not positions in
    /// [`DataStorage::records`], which [`DataStorage::parse`] sorts.
    pub fn stored_search(bytes: &[u8]) -> Result<Vec<SearchSchema>> {
        let h = file_header(bytes)?;
        parse_search(&segment_index(bytes, &h)?, h.search)
    }

    /// The default content (ODS 24.3, the content ODA writes for a section
    /// without data): the ACIS schema and four AcDbDs schemas, with no
    /// records.
    pub fn with_default_schemas() -> DataStorage {
        let flag = |name: &str| Schema {
            name: format!("{name}Schema"),
            properties: vec![SchemaProperty {
                name: name.into(),
                type_code: 1,
                ..SchemaProperty::default()
            }],
            ..Schema::default()
        };
        let mut schemas = vec![
            acis_schema(0),
            flag("AcDbDs::TreatedAsObjectData"),
            flag("AcDbDs::Legacy"),
            Schema {
                name: "AcDbDs::IndexedPropertySchema".into(),
                properties: vec![SchemaProperty {
                    name: "AcDs:Indexable".into(),
                    type_code: 1,
                    ..SchemaProperty::default()
                }],
                ..Schema::default()
            },
            Schema {
                name: "AcDbDs::HandleAttributeSchema".into(),
                properties: vec![SchemaProperty {
                    flags: 8,
                    name: "AcDbDs::HandleAttribute".into(),
                    type_code: 7,
                    unknown: 1,
                    value_count: 1,
                    values: vec![0],
                    ..SchemaProperty::default()
                }],
                ..Schema::default()
            },
        ];
        for (i, s) in schemas.iter_mut().enumerate() {
            s.index = i as u32;
        }
        // ODS 24.3 lists four, without indexes; AutoCAD numbers them from 1.
        let unknown_properties = [1, 1, 1, 0]
            .into_iter()
            .zip(1..)
            .map(|(flags, index)| UnknownProperty {
                index,
                data_size: 8,
                flags,
            })
            .collect();
        DataStorage {
            revision: 1,
            schemas,
            unknown_properties,
            records: Vec::new(),
        }
    }

    /// A hash of the whole value, which the drawing keeps instead of a copy
    /// of the value read to tell whether it changed (both hashes are taken
    /// in one process; `DefaultHasher::new` has fixed keys).
    ///
    /// The trade-off: keeping a 64-bit hash instead of a copy saves the
    /// memory of a second copy of every record (SAB data can be megabytes),
    /// at the price of a 2^-64 chance per changed value that a change goes
    /// unseen and the section read is written instead. The keys are fixed
    /// and known, so a value crafted in memory to collide could do that;
    /// the drawing's own user is the only one who can change it, and gains
    /// nothing by it.
    pub(crate) fn fingerprint(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.hash(&mut h);
        h.finish()
    }

    /// The index of the schema named `name`.
    pub fn schema_index(&self, name: &str) -> Option<u32> {
        self.schemas
            .iter()
            .find(|s| s.name == name)
            .map(|s| s.index)
    }

    /// The ACIS (SAB) data of entity `h`: its first record under
    /// [`ACIS_SCHEMA`], found by binary search in the records' data index
    /// order (see [`DataStorage::records`]).
    pub fn acis_data(&self, h: Handle) -> Option<&[u8]> {
        let schema = self.schema_index(ACIS_SCHEMA)?;
        let key = (schema, h);
        let at = self.records.partition_point(|r| {
            #[cfg(test)]
            ACIS_PROBES.with(|n| n.set(n.get() + 1));
            r.order() < key
        });
        let r = self.records.get(at).filter(|r| r.order() == key)?;
        Some(r.data.as_slice())
    }

    /// Put the records in data index order (by schema, then handle),
    /// stably: records of one handle keep their order.
    pub fn sort_records(&mut self) {
        self.records.sort_by_key(DataRecord::order);
    }

    /// Set entity `h`'s ACIS data: its records under [`ACIS_SCHEMA`] are
    /// replaced by one with `data`, placed in data index order (by schema,
    /// then handle). The schema is added (in ODS's form) when missing.
    /// Records not in that order (changed by hand) are sorted, stably.
    /// To set many, [`DataStorage::set_acis_data_all`] does it in one pass.
    pub fn set_acis_data(&mut self, h: Handle, data: Vec<u8>) {
        self.set_acis_data_all([(h, data)]);
    }

    /// [`DataStorage::set_acis_data`] for each `(handle, data)` of `all` (a
    /// handle given twice keeps its last data), in one pass over the
    /// records: the replaced ones go, the new ones are added and the
    /// records sorted once (stably).
    pub fn set_acis_data_all(&mut self, all: impl IntoIterator<Item = (Handle, Vec<u8>)>) {
        let mut new: HashMap<Handle, Vec<u8>> = HashMap::new();
        let mut order = Vec::new();
        for (h, data) in all {
            if new.insert(h, data).is_none() {
                order.push(h);
            }
        }
        if order.is_empty() {
            return;
        }
        let schema = match self.schema_index(ACIS_SCHEMA) {
            Some(s) => s,
            None => {
                // One past the largest index, or, when that would overflow
                // (an index of u32::MAX, from the file or JSON), the lowest
                // unused one.
                let index = if self.schemas.iter().any(|s| s.index == u32::MAX) {
                    let used: HashSet<u32> = self.schemas.iter().map(|s| s.index).collect();
                    (0..u32::MAX).find(|i| !used.contains(i)).unwrap_or(0)
                } else {
                    self.schemas.iter().map(|s| s.index + 1).max().unwrap_or(0)
                };
                self.schemas.push(acis_schema(index));
                index
            }
        };
        self.records
            .retain(|r| !(r.schema == schema && new.contains_key(&r.handle)));
        self.records.extend(order.into_iter().map(|h| DataRecord {
            handle: h,
            schema,
            data: new.remove(&h).unwrap_or_default(),
        }));
        self.sort_records();
    }

    /// Keep only the ACIS records whose handle `keep` accepts (records of
    /// other schemas stay).
    pub fn retain_acis(&mut self, mut keep: impl FnMut(Handle) -> bool) {
        if let Some(schema) = self.schema_index(ACIS_SCHEMA) {
            self.records
                .retain(|r| r.schema != schema || keep(r.handle));
        }
    }

    /// Change the handle records are keyed by, for a handle rewrite: called
    /// by `Drawing::rewrite_handles` (drawing/rewrite.rs) with the mapping it
    /// applies to the objects, so ACIS entities keep their data. `map` returns the new
    /// handle, or `None` to keep it. Records are then put back in data
    /// index order (stably).
    ///
    /// Fails with [`Error::Invalid`], changing nothing, when the mapping
    /// would key records of two different handles under one schema to the
    /// same handle (records that already shared a handle, as AutoCAD's
    /// thumbnails can, may keep it).
    pub fn remap_handles(&mut self, mut map: impl FnMut(Handle) -> Option<Handle>) -> Result<()> {
        let new: Vec<Handle> = self
            .records
            .iter()
            .map(|r| map(r.handle).unwrap_or(r.handle))
            .collect();
        let mut from: HashMap<(u32, Handle), Handle> = HashMap::with_capacity(new.len());
        for (r, &h) in self.records.iter().zip(&new) {
            match from.insert((r.schema, h), r.handle) {
                Some(old) if old != r.handle => {
                    return Err(crate::error::invalid(format!(
                        "records of {old} and {} would both be keyed by {h}",
                        r.handle
                    )));
                }
                _ => {}
            }
        }
        for (r, h) in self.records.iter_mut().zip(new) {
            r.handle = h;
        }
        self.sort_records();
        Ok(())
    }

    /// The search data [`DataStorage::encode`] writes: for each schema
    /// with records, in schema order, the data index positions of its
    /// records, and one set of its handles (ascending) with their
    /// positions.
    pub fn search(&self) -> Vec<SearchSchema> {
        // Per schema: the positions of its records, and of each handle's.
        type Positions = (Vec<u64>, BTreeMap<Handle, Vec<u64>>);
        let mut by_schema: BTreeMap<u32, Positions> = BTreeMap::new();
        for (i, r) in self.records.iter().enumerate() {
            let (sorted, ids) = by_schema.entry(r.schema).or_default();
            sorted.push(i as u64);
            ids.entry(r.handle).or_default().push(i as u64);
        }
        by_schema
            .into_iter()
            .map(|(schema, (sorted, ids))| SearchSchema {
                schema,
                sorted,
                ids: vec![ids.into_iter().collect()],
            })
            .collect()
    }

    /// Write the section in a fresh layout (see the module documentation).
    pub fn encode(&self) -> Result<Vec<u8>> {
        // Segment numbers; blob pages follow the search segment.
        const SEGIDX: u32 = 1;
        const DATIDX: u32 = 2;
        const DATA: u32 = 3;
        const SCHIDX: u32 = 4;
        const SCHDAT: u32 = 5;
        const SEARCH: u32 = 6;
        let mut segments: Vec<SegmentOut> = Vec::new();

        // Data: the record headers, padding to a 16-byte boundary, then the
        // records.
        let mut blobs: Vec<Vec<u8>> = Vec::new();
        let mut headers = Vec::new();
        let mut data = Vec::new();
        let mut index_entries = Vec::new();
        for r in &self.records {
            index_entries.push((DATA, headers.len() as u32, r.schema));
            headers.extend_from_slice(&RECORD_HEADER_LEN.to_le_bytes());
            headers.extend_from_slice(&1u32.to_le_bytes());
            headers.extend_from_slice(&r.handle.0.to_le_bytes());
            headers.extend_from_slice(&len32(data.len())?.to_le_bytes());
            if r.data.len() <= INLINE_MAX {
                data.extend_from_slice(&len32(r.data.len())?.to_le_bytes());
                data.extend_from_slice(&r.data);
            } else {
                let pages: Vec<&[u8]> = r.data.chunks(BLOB_PAGE_MAX).collect();
                let first = SEARCH + 1 + blobs.len() as u32;
                data.extend_from_slice(&BLOB_MARKER.to_le_bytes());
                data.extend_from_slice(&(r.data.len() as u64).to_le_bytes());
                data.extend_from_slice(&len32(pages.len())?.to_le_bytes());
                data.extend_from_slice(&len32(32 + 8 * pages.len())?.to_le_bytes());
                data.extend_from_slice(&len32(pages[0].len())?.to_le_bytes());
                data.extend_from_slice(&len32(pages[pages.len() - 1].len())?.to_le_bytes());
                data.extend_from_slice(&[0; 8]);
                let mut start = 0u64;
                for (i, page) in pages.iter().enumerate() {
                    let mut body = Vec::with_capacity(page.len() + 32);
                    body.extend_from_slice(&(r.data.len() as u64).to_le_bytes());
                    body.extend_from_slice(&start.to_le_bytes());
                    body.extend_from_slice(&len32(i)?.to_le_bytes());
                    body.extend_from_slice(&len32(pages.len())?.to_le_bytes());
                    body.extend_from_slice(&(page.len() as u64).to_le_bytes());
                    body.extend_from_slice(page);
                    start += page.len() as u64;
                    data.extend_from_slice(&(first + i as u32).to_le_bytes());
                    data.extend_from_slice(&len32(segment_size(body.len()))?.to_le_bytes());
                    blobs.push(body);
                }
            }
        }

        let mut datidx = Vec::with_capacity(8 + 12 * index_entries.len());
        datidx.extend_from_slice(&len32(index_entries.len())?.to_le_bytes());
        datidx.extend_from_slice(&0u32.to_le_bytes());
        for (seg, offset, schema) in &index_entries {
            for v in [seg, offset, schema] {
                datidx.extend_from_slice(&v.to_le_bytes());
            }
        }

        // Schema data: the unknown properties, the schemas, then (aligned)
        // the property names.
        let mut names = NameTable::default();
        let mut schdat = Vec::new();
        let mut prop_entries = Vec::new();
        for p in &self.unknown_properties {
            prop_entries.push((SCHDAT, len32(schdat.len())?, p.index));
            schdat.extend_from_slice(&p.data_size.to_le_bytes());
            schdat.extend_from_slice(&p.flags.to_le_bytes());
        }
        let mut schema_entries = Vec::new();
        for s in &self.schemas {
            schema_entries.push((s.index, SCHDAT, len32(schdat.len())?));
            write_schema(&mut schdat, s, &mut names)?;
        }
        let schdat_names = align_table(&mut schdat);
        write_names(&mut schdat, names.names.iter().copied())?;

        let mut schidx = Vec::new();
        schidx.extend_from_slice(&len32(schema_entries.len())?.to_le_bytes());
        schidx.extend_from_slice(&0u32.to_le_bytes());
        for (index, seg, offset) in &schema_entries {
            for v in [index, seg, offset] {
                schidx.extend_from_slice(&v.to_le_bytes());
            }
        }
        schidx.extend_from_slice(&SCHEMA_INDEX_MAGIC.to_le_bytes());
        schidx.extend_from_slice(&len32(prop_entries.len())?.to_le_bytes());
        schidx.extend_from_slice(&0u32.to_le_bytes());
        for (seg, offset, index) in &prop_entries {
            for v in [seg, offset, index] {
                schidx.extend_from_slice(&v.to_le_bytes());
            }
        }
        let schidx_names = align_table(&mut schidx);
        write_names(&mut schidx, self.schemas.iter().map(|s| s.name.as_str()))?;

        let mut search = Vec::new();
        let entries = self.search();
        search.extend_from_slice(&len32(entries.len())?.to_le_bytes());
        for e in &entries {
            search.extend_from_slice(&e.schema.to_le_bytes());
            search.extend_from_slice(&(e.sorted.len() as u64).to_le_bytes());
            e.sorted
                .iter()
                .for_each(|i| search.extend_from_slice(&i.to_le_bytes()));
            search.extend_from_slice(&len32(e.ids.len())?.to_le_bytes());
            if !e.ids.is_empty() {
                search.extend_from_slice(&0u32.to_le_bytes());
            }
            for set in &e.ids {
                search.extend_from_slice(&len32(set.len())?.to_le_bytes());
                for (h, indexes) in set {
                    search.extend_from_slice(&h.0.to_le_bytes());
                    search.extend_from_slice(&(indexes.len() as u64).to_le_bytes());
                    indexes
                        .iter()
                        .for_each(|i| search.extend_from_slice(&i.to_le_bytes()));
                }
            }
        }

        let mut data_body = headers;
        let data_start = align_with(&mut data_body, DATA_PAD);
        data_body.extend_from_slice(&data);

        let count = SEARCH as usize + 1 + blobs.len();
        segments.push(SegmentOut::new(SEGIDX, *b"segidx", vec![0; 12 * count]));
        segments.push(SegmentOut::new(DATIDX, *b"datidx", datidx));
        let mut d = SegmentOut::new(DATA, *b"_data_", data_body);
        d.obj_align = data_start;
        segments.push(d);
        for (i, body) in blobs.into_iter().enumerate() {
            let mut b = SegmentOut::new(SEARCH + 1 + i as u32, *b"blob01", body);
            b.unknown1 = 1;
            segments.push(b);
        }
        let mut s = SegmentOut::new(SCHIDX, *b"schidx", schidx);
        s.sys_align = schidx_names;
        segments.push(s);
        let mut s = SegmentOut::new(SCHDAT, *b"schdat", schdat);
        s.sys_align = schdat_names;
        segments.push(s);
        segments.push(SegmentOut::new(SEARCH, *b"search", search));

        // Offsets, then the segment index's entries.
        let mut table = vec![(0u64, 0u32); count];
        let mut at = FILE_HEADER_LEN;
        for s in &segments {
            let size = segment_size(s.body.len());
            table[s.index as usize] = (at as u64, len32(size)?);
            at = at
                .checked_add(size)
                .ok_or_else(|| encode_err("too large"))?;
        }
        let file_size = len32(at)?;
        let mut entries = Vec::with_capacity(12 * count);
        for (offset, size) in &table {
            entries.extend_from_slice(&offset.to_le_bytes());
            entries.extend_from_slice(&size.to_le_bytes());
        }
        segments[0].body = entries;

        let mut out = Vec::with_capacity(at);
        for v in [
            SIGNATURE,
            HEADER_SIZE_FIELD,
            HEADER_UNKNOWN_1,
            HEADER_VERSION,
            0,
            self.revision,
            len32(table[SEGIDX as usize].0 as usize)?,
            0,
            len32(count)?,
            SCHIDX,
            DATIDX,
            SEARCH,
            0, // previous save
            file_size,
            0,
            0, // free space segment
            0, // free space count
        ] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out.resize(FILE_HEADER_LEN, 0);
        for s in &segments {
            s.write(&mut out, self.revision)?;
        }
        Ok(out)
    }
}

/// The ACIS schema as ODS 24.3 gives it: an `AcDbDs::ID` (type 10, values 2
/// and 3) and the `ASM_Data` (type 15).
fn acis_schema(index: u32) -> Schema {
    let ids: Vec<u8> = [2u64, 3].iter().flat_map(|v| v.to_le_bytes()).collect();
    Schema {
        index,
        name: ACIS_SCHEMA.into(),
        indexes: vec![0, 1],
        properties: vec![
            SchemaProperty {
                name: "AcDbDs::ID".into(),
                type_code: 10,
                value_count: 2,
                values: ids,
                ..SchemaProperty::default()
            },
            SchemaProperty {
                name: "ASM_Data".into(),
                type_code: 15,
                ..SchemaProperty::default()
            },
        ],
    }
}

fn len32(n: usize) -> Result<u32> {
    u32::try_from(n).map_err(|_| encode_err("data too large"))
}

/// A segment's size: header and body, padded to [`SEGMENT_ALIGN`].
fn segment_size(body: usize) -> usize {
    (SEGMENT_HEADER_LEN + body).div_ceil(SEGMENT_ALIGN) * SEGMENT_ALIGN
}

/// Pad `body` with `pad` to a 16-byte boundary; returns the position as an
/// alignment offset (x 16 from the segment start).
fn align_with(body: &mut Vec<u8>, pad: u8) -> usize {
    body.resize(body.len().next_multiple_of(16), pad);
    (SEGMENT_HEADER_LEN + body.len()) / 16
}

fn align_table(body: &mut Vec<u8>) -> usize {
    align_with(body, TABLE_PAD)
}

fn latin1(s: &str) -> Result<Vec<u8>> {
    s.chars()
        .map(|c| u8::try_from(u32::from(c)).ok().filter(|&b| b != 0))
        .collect::<Option<_>>()
        .ok_or_else(|| encode_err(format!("name {s:?} is not Latin-1")))
}

fn write_names<'a>(out: &mut Vec<u8>, names: impl ExactSizeIterator<Item = &'a str>) -> Result<()> {
    out.extend_from_slice(&len32(names.len())?.to_le_bytes());
    for n in names {
        out.extend_from_slice(&latin1(n)?);
        out.push(0);
    }
    Ok(())
}

/// The property names of the schema data segment, in first-use order, with
/// each one's index.
#[derive(Default)]
struct NameTable<'a> {
    names: Vec<&'a str>,
    index: HashMap<&'a str, usize>,
}

impl<'a> NameTable<'a> {
    /// The index of `name`, added at the end when new.
    fn index_of(&mut self, name: &'a str) -> usize {
        *self.index.entry(name).or_insert_with(|| {
            self.names.push(name);
            self.names.len() - 1
        })
    }
}

fn write_schema<'a>(out: &mut Vec<u8>, s: &'a Schema, names: &mut NameTable<'a>) -> Result<()> {
    let count = |n: usize| u16::try_from(n).map_err(|_| encode_err("schema too large"));
    out.extend_from_slice(&count(s.indexes.len())?.to_le_bytes());
    s.indexes
        .iter()
        .for_each(|i| out.extend_from_slice(&i.to_le_bytes()));
    out.extend_from_slice(&count(s.properties.len())?.to_le_bytes());
    for p in &s.properties {
        let name = names.index_of(&p.name);
        out.extend_from_slice(&p.flags.to_le_bytes());
        out.extend_from_slice(&len32(name)?.to_le_bytes());
        if p.has_type() {
            out.extend_from_slice(&p.type_code.to_le_bytes());
            if p.type_code == CUSTOM_TYPE {
                out.extend_from_slice(&p.custom_size.to_le_bytes());
            }
        }
        if p.flags == 1 || p.flags == 8 {
            out.extend_from_slice(&p.unknown.to_le_bytes());
        }
        let size = p.value_size()?;
        if p.values.len() != size * usize::from(p.value_count) {
            return Err(encode_err(format!(
                "property {} has {} value bytes for {} values of {size}",
                p.name,
                p.values.len(),
                p.value_count
            )));
        }
        out.extend_from_slice(&p.value_count.to_le_bytes());
        out.extend_from_slice(&p.values);
    }
    Ok(())
}

/// A segment to write.
struct SegmentOut {
    index: u32,
    name: [u8; 6],
    unknown1: u32,
    sys_align: usize,
    obj_align: usize,
    body: Vec<u8>,
}

impl SegmentOut {
    fn new(index: u32, name: [u8; 6], body: Vec<u8>) -> Self {
        SegmentOut {
            index,
            name,
            unknown1: 0,
            sys_align: 0,
            obj_align: 0,
            body,
        }
    }

    fn write(&self, out: &mut Vec<u8>, revision: u32) -> Result<()> {
        let size = segment_size(self.body.len());
        let start = out.len();
        out.extend_from_slice(&SEGMENT_SIGNATURE.to_le_bytes());
        out.extend_from_slice(&self.name);
        for v in [
            self.index,
            self.unknown1,
            len32(size)?,
            0,
            revision,
            0,
            len32(self.sys_align)?,
            len32(self.obj_align)?,
        ] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out.extend_from_slice(&ALIGNMENT_BYTES);
        out.extend_from_slice(&self.body);
        out.resize(start + size, SEGMENT_PAD);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(h: u64, schema: u32, len: usize) -> DataRecord {
        DataRecord {
            handle: Handle(h),
            schema,
            data: (0..len).map(|i| (i * 7 + h as usize) as u8).collect(),
        }
    }

    fn sample() -> DataStorage {
        let mut s = DataStorage::with_default_schemas();
        s.records = vec![record(0x20, 0, 100), record(0x30, 0, 5), record(0x30, 3, 0)];
        s
    }

    /// A schema index of u32::MAX (hostile input) leaves no "one past the
    /// largest": the ACIS schema takes the lowest unused index instead of
    /// overflowing.
    #[test]
    fn acis_schema_after_max_index_takes_lowest_unused() {
        let mut s = DataStorage::with_default_schemas();
        s.schemas.retain(|x| x.name != ACIS_SCHEMA);
        let mut odd = s.schemas[0].clone();
        odd.index = u32::MAX;
        odd.name = "Odd".into();
        s.schemas.push(odd);
        s.set_acis_data(Handle(0x40), vec![1, 2, 3]);
        let indexes: HashSet<u32> = s.schemas.iter().map(|x| x.index).collect();
        assert_eq!(indexes.len(), s.schemas.len(), "schema indexes stay unique");
        let acis = s.schema_index(ACIS_SCHEMA).unwrap();
        assert_ne!(acis, u32::MAX);
        assert_eq!(s.acis_data(Handle(0x40)), Some(&[1u8, 2, 3][..]));
    }

    #[test]
    fn roundtrip_default_and_records() {
        let empty = DataStorage::with_default_schemas();
        assert_eq!(DataStorage::parse(&empty.encode().unwrap()).unwrap(), empty);
        let s = sample();
        let bytes = s.encode().unwrap();
        assert_eq!(bytes.len() % SEGMENT_ALIGN, 0);
        assert_eq!(DataStorage::parse(&bytes).unwrap(), s);
        assert_eq!(DataStorage::stored_search(&bytes).unwrap(), s.search());
    }

    /// ODS 24.3's default schemas, as the corpus's AutoCAD files have them
    /// where they share them (the AcDbDs schemas).
    #[test]
    fn default_schemas_follow_ods() {
        let s = DataStorage::with_default_schemas();
        let names: Vec<&str> = s.schemas.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(
            names,
            [
                ACIS_SCHEMA,
                "AcDbDs::TreatedAsObjectDataSchema",
                "AcDbDs::LegacySchema",
                "AcDbDs::IndexedPropertySchema",
                "AcDbDs::HandleAttributeSchema"
            ]
        );
        assert_eq!(s.schema_index(ACIS_SCHEMA), Some(0));
    }

    /// Records over 0x40000 bytes are stored as blob pages (ODS 24.2.2.3.1).
    #[test]
    fn large_records_are_blobs() {
        let mut s = DataStorage::with_default_schemas();
        s.set_acis_data(Handle(0x40), record(0x40, 0, 2 * BLOB_PAGE_MAX + 17).data);
        s.set_acis_data(Handle(0x41), record(0x41, 0, INLINE_MAX + 1).data);
        s.set_acis_data(Handle(0x3F), record(0x3F, 0, INLINE_MAX).data);
        let bytes = s.encode().unwrap();
        let blobs = bytes
            .windows(8)
            .filter(|w| w[2..] == *b"blob01" && w[..2] == SEGMENT_SIGNATURE.to_le_bytes())
            .count();
        assert_eq!(blobs, 4);
        assert_eq!(DataStorage::parse(&bytes).unwrap(), s);
    }

    #[test]
    fn acis_records_by_handle() {
        let mut s = DataStorage::default();
        s.set_acis_data(Handle(0x50), vec![1]);
        assert_eq!(s.schema_index(ACIS_SCHEMA), Some(0));
        s.set_acis_data(Handle(0x40), vec![2]);
        s.set_acis_data(Handle(0x50), vec![3]);
        let handles: Vec<u64> = s.records.iter().map(|r| r.handle.0).collect();
        assert_eq!(handles, [0x40, 0x50]);
        assert_eq!(s.acis_data(Handle(0x50)), Some(&[3][..]));
        s.remap_handles(|h| (h == Handle(0x40)).then_some(Handle(0x60)))
            .unwrap();
        // Two handles onto one: refused, nothing changed.
        let before = s.clone();
        assert!(s.remap_handles(|_| Some(Handle(0x70))).is_err());
        assert_eq!(s, before);
        // Out of order by hand: set_acis_data sorts first.
        s.records.reverse();
        s.set_acis_data(Handle(0x55), vec![9]);
        let order: Vec<u64> = s.records.iter().map(|r| r.handle.0).collect();
        assert_eq!(order, [0x50, 0x55, 0x60]);
        s.records.retain(|r| r.handle != Handle(0x55));
        let handles: Vec<u64> = s.records.iter().map(|r| r.handle.0).collect();
        assert_eq!(handles, [0x50, 0x60]);
        s.retain_acis(|h| h != Handle(0x50));
        assert_eq!(s.records.len(), 1);
        assert_eq!(s.acis_data(Handle(0x60)), Some(&[2][..]));
    }

    /// A record is found by binary search, at most about log2(n) + 2
    /// comparisons for n records; a handle with no record, or a record
    /// under another schema, is not found; of a handle's records the first
    /// is. Records out of data index order (read so, deserialized, or
    /// changed by hand and sorted) are put back in it, stably.
    #[test]
    fn acis_data_found_by_binary_search() {
        let mut s = DataStorage::with_default_schemas();
        let acis = s.schema_index(ACIS_SCHEMA).unwrap();
        let other = s.schema_index("AcDbDs::LegacySchema").unwrap();
        s.set_acis_data_all((1..=1000u64).map(|i| (Handle(2 * i), vec![i as u8])));
        s.records.push(DataRecord {
            handle: Handle(3),
            schema: other,
            data: vec![0xEE],
        });
        s.sort_records();
        let n = s.records.len();
        let per = n.ilog2() as usize + 2;
        let check = |s: &DataStorage| {
            for i in [1u64, 2, 500, 999, 1000] {
                ACIS_PROBES.with(|p| p.set(0));
                assert_eq!(s.acis_data(Handle(2 * i)), Some(&[i as u8][..]), "{i}");
                let probes = ACIS_PROBES.with(|p| p.get());
                assert!(probes <= per, "{probes} comparisons for {n} records");
            }
            assert_eq!(s.acis_data(Handle(3)), None);
            assert_eq!(s.acis_data(Handle(2001)), None);
            assert_eq!(s.acis_data(Handle(0)), None);
        };
        check(&s);
        let sorted = s.records.clone();
        s.records.reverse();
        s.records.rotate_left(337);
        s.sort_records();
        assert_eq!(s.records, sorted);
        check(&s);
        // Parsing sorts records stored out of order.
        let mut unsorted = s.clone();
        unsorted.records.reverse();
        let parsed = DataStorage::parse(&unsorted.encode().unwrap()).unwrap();
        assert_eq!(parsed.records, sorted);
        // Two records of one handle: the first, in stored order.
        let dup = |data: u8| DataRecord {
            handle: Handle(0x11),
            schema: acis,
            data: vec![data],
        };
        s.records.push(dup(0xA1));
        s.records.push(dup(0xA2));
        s.sort_records();
        assert_eq!(s.acis_data(Handle(0x11)), Some(&[0xA1][..]));
    }

    /// Setting many records at once gives what setting them one by one
    /// gives: a handle twice keeps its last data, records of other schemas
    /// and handles stay, out-of-order records are sorted stably; nothing to
    /// set adds no schema.
    #[test]
    fn acis_records_set_at_once() {
        let mut base = DataStorage::with_default_schemas();
        let other = base.schema_index("AcDbDs::LegacySchema").unwrap();
        base.set_acis_data(Handle(0x30), vec![1]);
        base.set_acis_data(Handle(0x50), vec![2]);
        base.records.push(DataRecord {
            handle: Handle(0x40),
            schema: other,
            data: vec![3],
        });
        base.records.reverse();
        let batch = [
            (Handle(0x50), vec![4]),
            (Handle(0x20), vec![5]),
            (Handle(0x45), vec![6]),
            (Handle(0x20), vec![7]),
        ];
        let mut one_by_one = base.clone();
        for (h, data) in batch.clone() {
            one_by_one.set_acis_data(h, data);
        }
        let mut at_once = base.clone();
        at_once.set_acis_data_all(batch);
        assert_eq!(at_once, one_by_one);
        assert_eq!(at_once.acis_data(Handle(0x20)), Some(&[7][..]));
        let mut none = DataStorage::default();
        none.set_acis_data_all([]);
        assert_eq!(none, DataStorage::default());
    }

    /// Every prefix and many corruptions of a section fail cleanly; counts
    /// and offsets are checked before anything is allocated.
    #[test]
    fn hostile_sections() {
        let bytes = sample().encode().unwrap();
        for n in 0..bytes.len() {
            let _ = DataStorage::parse(&bytes[..n]);
            let _ = DataStorage::stored_search(&bytes[..n]);
        }
        for at in (0..bytes.len()).step_by(4) {
            for v in [0u32, 1, 0x7FFF_FFFF, u32::MAX, BLOB_MARKER] {
                let mut b = bytes.clone();
                b[at..at + 4].copy_from_slice(&v.to_le_bytes());
                let _ = DataStorage::parse(&b);
                let _ = DataStorage::stored_search(&b);
            }
        }
    }

    /// Absurd counts are refused before allocating.
    #[test]
    fn absurd_counts() {
        let bytes = sample().encode().unwrap();
        // The segment index count in the file header.
        let mut b = bytes.clone();
        b[32..36].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(DataStorage::parse(&b).is_err());
        // The data index count (the data index is segment 2, right after
        // the segment index).
        let segidx_size = u32::from_le_bytes(bytes[0x80 + 16..0x80 + 20].try_into().unwrap());
        let datidx = 0x80 + segidx_size as usize + SEGMENT_HEADER_LEN;
        let mut b = bytes.clone();
        b[datidx..datidx + 4].copy_from_slice(&0x1000_0000u32.to_le_bytes());
        assert!(matches!(DataStorage::parse(&b), Err(Error::Corrupt(_))));
        // A segment offset past the end.
        let mut b = bytes.clone();
        b[24..28].copy_from_slice(&0x7000_0000u32.to_le_bytes());
        assert!(DataStorage::parse(&b).is_err());
    }

    /// The body offset of segment `index` of `bytes`.
    fn segment_body(bytes: &[u8], index: u32) -> usize {
        let h = file_header(bytes).unwrap();
        segment_index(bytes, &h).unwrap().get(index).unwrap().body()
    }

    /// Index entries all pointing at one large item would copy it once per
    /// entry: the copies are charged against twice the section's size, so
    /// such a section fails instead of exhausting memory.
    #[test]
    fn repeated_items_are_bounded() {
        // 300 records; every data index entry then names the first, of
        // 50 000 bytes.
        let mut s = DataStorage::with_default_schemas();
        s.records = (0..300).map(|h| record(0x100 + h, 0, 1)).collect();
        s.records[0].data = vec![7; 50_000];
        let mut bytes = s.encode().unwrap();
        assert_eq!(DataStorage::parse(&bytes).unwrap(), s);
        let datidx = segment_body(&bytes, 2);
        for k in 0..300 {
            let at = datidx + 8 + 12 * k + 4;
            bytes[at..at + 4].copy_from_slice(&0u32.to_le_bytes());
        }
        assert!(
            matches!(DataStorage::parse(&bytes), Err(Error::Corrupt(m)) if m.contains("repeat"))
        );

        // 300 schemas; every schema index entry then names the first, whose
        // property has a 20 000-character name.
        let mut s = DataStorage {
            schemas: (0..300)
                .map(|i| Schema {
                    index: i,
                    name: format!("S{i}"),
                    ..Schema::default()
                })
                .collect(),
            ..DataStorage::default()
        };
        s.schemas[0].properties.push(SchemaProperty {
            name: "N".repeat(20_000),
            type_code: 1,
            ..SchemaProperty::default()
        });
        let mut bytes = s.encode().unwrap();
        assert_eq!(DataStorage::parse(&bytes).unwrap(), s);
        let schidx = segment_body(&bytes, 4);
        for k in 0..300 {
            let at = schidx + 8 + 12 * k + 8;
            bytes[at..at + 4].copy_from_slice(&0u32.to_le_bytes());
        }
        assert!(
            matches!(DataStorage::parse(&bytes), Err(Error::Corrupt(m)) if m.contains("repeat"))
        );
    }

    /// Many segment index entries naming one schema data segment (whose
    /// name table is large): its names are copied once per extent and
    /// charged to the budget, so aliasing costs no more than the section.
    #[test]
    fn aliased_segments_are_bounded() {
        let mut s = DataStorage {
            schemas: (0..200)
                .map(|i| Schema {
                    index: i,
                    name: format!("S{i}"),
                    properties: vec![SchemaProperty {
                        name: format!("{i}{}", "N".repeat(100)),
                        type_code: 1,
                        ..SchemaProperty::default()
                    }],
                    ..Schema::default()
                })
                .collect(),
            ..DataStorage::default()
        };
        s.revision = 1;
        let bytes = s.encode().unwrap();
        assert_eq!(DataStorage::parse(&bytes).unwrap(), s);
        // Give the segment index 2000 more entries, all aliasing the
        // schema data segment (5), and point each schema at its own alias:
        // without the per-extent cache, 200 tables of 200 names.
        let h = file_header(&bytes).unwrap();
        let segs = segment_index(&bytes, &h).unwrap();
        let schdat = segs.entries[5];
        let count = segs.entries.len();
        drop(segs);
        let mut out = bytes.clone();
        let extra = 2000usize;
        let segidx = h.segidx_offset as usize;
        // A new segment index at the end, holding the old entries and the
        // aliases.
        let mut body = Vec::new();
        let mut entries = Vec::new();
        for i in 0..count {
            let at = segidx + SEGMENT_HEADER_LEN + 12 * i;
            entries.extend_from_slice(&bytes[at..at + 12]);
        }
        for _ in 0..extra {
            entries.extend_from_slice(&schdat.0.to_le_bytes());
            entries.extend_from_slice(&schdat.1.to_le_bytes());
        }
        let start = out.len();
        let seg = SegmentOut::new(1, *b"segidx", entries);
        seg.write(&mut body, 1).unwrap();
        out.extend_from_slice(&body);
        out[24..28].copy_from_slice(&(start as u32).to_le_bytes());
        out[32..36].copy_from_slice(&((count + extra) as u32).to_le_bytes());
        let schidx = {
            let h = file_header(&out).unwrap();
            segment_index(&out, &h).unwrap().get(4).unwrap().body()
        };
        for k in 0..200 {
            let at = schidx + 8 + 12 * k + 4;
            out[at..at + 4].copy_from_slice(&((count + k * 10) as u32).to_le_bytes());
        }
        let parsed = DataStorage::parse(&out).unwrap();
        assert_eq!(parsed, s);
        // Aliases that differ in size (each one byte longer than the last)
        // are distinct extents, each parsed: the budget stops the copies.
        let mut distinct = out.clone();
        let segidx_body = start + SEGMENT_HEADER_LEN;
        for k in 0..extra {
            let at = segidx_body + 12 * (count + k) + 8;
            distinct[at..at + 4].copy_from_slice(&(schdat.1 + k as u32).to_le_bytes());
        }
        assert!(matches!(
            DataStorage::parse(&distinct),
            Err(Error::Corrupt(m)) if m.contains("repeat")
        ));
    }

    #[test]
    fn bad_property_values_are_refused() {
        let mut s = DataStorage::with_default_schemas();
        s.schemas[0].properties[0].value_count = 3;
        assert!(matches!(s.encode(), Err(Error::Encode(_))));
        let mut s = DataStorage::with_default_schemas();
        s.schemas[0].name = "\u{100}".into();
        assert!(matches!(s.encode(), Err(Error::Encode(_))));
    }
}
