//! Groups (ODS 20.4.72), scales (20.4.92), sort tables (20.4.93), spatial
//! filters and indexes (20.4.94, 20.4.95) and layer indexes (20.4.83).

use crate::error::Result;
use crate::error::encode_error as encode_err;
use crate::handle::{Handle, HandleRef};
use crate::object::entities::dim::{
    MIN_BL, check_fits, check_handles, corrupt, read_count, write_count,
};
use crate::object::raw::{HandleTail, take_rest, trailing_refs};
use crate::object::stream::{ObjReader, ObjWriter};
use crate::object::tables::{HARD_POINTER, SOFT_POINTER, handle_count, put};
use crate::version::Version;

/// A group (ODS 20.4.72): a named selection of entities.
#[derive(Debug, Clone, PartialEq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Group {
    /// The description (DXF 300).
    pub name: String,
    /// DXF 70: 1 when the group is unnamed.
    pub unnamed: i16,
    /// DXF 71: 1 when the group is selectable.
    pub selectable: i16,
    /// The entities (hard pointers). Each entity lists the group among its
    /// reactors; keeping the two sides consistent is a cross-object check
    /// (Task 23's validator), not done here.
    pub entities: Vec<Handle>,
}

impl Group {
    pub(crate) fn decode(r: &mut ObjReader) -> Result<Self> {
        let name = r.text()?;
        let unnamed = r.data.read_bs()?;
        let selectable = r.data.read_bs()?;
        let n = r.data.read_bl()?;
        let n = handle_count(r, n, "group entity")?;
        let entities = (0..n).map(|_| r.href_abs()).collect::<Result<_>>()?;
        Ok(Group {
            name,
            unnamed,
            selectable,
            entities,
        })
    }

    pub(crate) fn encode(&self, w: &mut ObjWriter) -> Result<()> {
        w.text(&self.name)?;
        w.data.write_bs(self.unnamed);
        w.data.write_bs(self.selectable);
        write_count(w, self.entities.len(), "group entities")?;
        for &h in &self.entities {
            put(w, HARD_POINTER, h)?;
        }
        Ok(())
    }
}

/// An annotation scale (ODS 20.4.92, SCALE).
#[derive(Debug, Clone, PartialEq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Scale {
    /// DXF 70 (ODA writes 0).
    pub unknown: i16,
    /// DXF 300.
    pub name: String,
    /// DXF 140.
    pub paper_units: f64,
    /// DXF 141.
    pub drawing_units: f64,
    /// DXF 290.
    pub unit_scale: bool,
}

impl Scale {
    pub(crate) fn decode(r: &mut ObjReader) -> Result<Self> {
        Ok(Scale {
            unknown: r.data.read_bs()?,
            name: r.text()?,
            paper_units: r.data.read_bd()?,
            drawing_units: r.data.read_bd()?,
            unit_scale: r.data.read_b()?,
        })
    }

    pub(crate) fn encode(&self, w: &mut ObjWriter) -> Result<()> {
        w.data.write_bs(self.unknown);
        w.text(&self.name)?;
        w.data.write_bd(self.paper_units);
        w.data.write_bd(self.drawing_units);
        w.data.write_b(self.unit_scale);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Sort tables.

/// A sort table (ODS 20.4.93, SORTENTSTABLE): the draw order of a block's
/// entities.
#[derive(Debug, Clone, PartialEq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SortEntsTable {
    /// The block record whose entities are sorted (soft pointer).
    pub block: Handle,
    /// (sort handle, entity) pairs. The sort handle is stored in the data
    /// stream (ODS: "CODE 0"); the entity is a soft pointer.
    pub entries: Vec<(Handle, Handle)>,
}

impl SortEntsTable {
    pub(crate) fn decode(r: &mut ObjReader) -> Result<Self> {
        let n = r.data.read_bl()?;
        let block = r.href_abs()?;
        let n = handle_count(r, n, "sort table entry")?;
        // Each sort handle takes at least a byte of the data stream.
        check_fits(r, n, 8, "sort table entries")?;
        let mut entries = Vec::with_capacity(n);
        for _ in 0..n {
            let sort = r.data.read_handle_ref()?;
            if sort.code != 0 {
                return Err(corrupt(format!(
                    "sort handle reference code {} (expected 0)",
                    sort.code
                )));
            }
            entries.push((Handle(sort.value), r.href_abs()?));
        }
        Ok(SortEntsTable { block, entries })
    }

    pub(crate) fn encode(&self, w: &mut ObjWriter) -> Result<()> {
        write_count(w, self.entries.len(), "sort table entries")?;
        put(w, SOFT_POINTER, self.block)?;
        for &(sort, entity) in &self.entries {
            w.data.write_handle_ref(HandleRef {
                code: 0,
                value: sort.0,
            })?;
            put(w, SOFT_POINTER, entity)?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Indexes and filters.

/// Bits of an object kept as stored: the data stream bits and, from R2007,
/// the string stream bits.
#[derive(Debug, Clone, PartialEq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct OpaqueBits {
    /// MSB-first, zero-padded.
    pub data: Vec<u8>,
    pub data_bits: u64,
    /// R2007+: the string stream bits; `None` when empty.
    pub strings: Option<(Vec<u8>, u64)>,
}

impl OpaqueBits {
    fn take(r: &mut ObjReader) -> Result<Self> {
        let (data, data_bits) = take_rest(&mut r.data)?;
        let strings = match r.strings.as_mut() {
            Some(s) => Some(take_rest(s)?).filter(|(_, bits)| *bits > 0),
            None => None,
        };
        Ok(OpaqueBits {
            data,
            data_bits,
            strings,
        })
    }

    fn write(&self, w: &mut ObjWriter) -> Result<()> {
        w.data.write_bits_from(&self.data, self.data_bits)?;
        if let Some((bytes, bits)) = &self.strings {
            w.write_raw_strings(bytes, *bits)?;
        }
        Ok(())
    }
}

/// A spatial filter (ODS 20.4.94, SPATIAL_FILTER), the clip boundary of an
/// xref or block reference. Kept typed-opaque: its data bits as stored
/// (they have no text, so they are the same in every version) and any
/// references after the common ones.
#[derive(Debug, Clone, PartialEq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct SpatialFilter {
    pub bits: OpaqueBits,
    pub handles: Vec<HandleRef>,
    /// Handle stream bits after the last reference that do not parse as
    /// one, kept as stored (see [`crate::object::HandleTail`]).
    #[cfg_attr(feature = "serde", serde(default))]
    pub handle_tail: HandleTail,
}

impl SpatialFilter {
    pub(crate) fn decode(r: &mut ObjReader) -> Result<Self> {
        let bits = OpaqueBits::take(r)?;
        let (handles, handle_tail) = trailing_refs(r)?;
        Ok(SpatialFilter {
            bits,
            handles,
            handle_tail,
        })
    }

    pub(crate) fn encode(&self, w: &mut ObjWriter) -> Result<()> {
        self.bits.write(w)?;
        for &h in &self.handles {
            w.href(h)?;
        }
        self.handle_tail.write(w)
    }
}

/// A spatial index (ODS 20.4.95, SPATIAL_INDEX). Kept typed-opaque: the two
/// time stamps, then the bits ODS calls unknown as stored and any references
/// after the common ones.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct SpatialIndex {
    pub timestamps: [i32; 2],
    pub bits: OpaqueBits,
    pub handles: Vec<HandleRef>,
    /// Handle stream bits after the last reference that do not parse as
    /// one, kept as stored (see [`crate::object::HandleTail`]).
    #[cfg_attr(feature = "serde", serde(default))]
    pub handle_tail: HandleTail,
    /// The version the bits were read from. Bits of unknown layout are only
    /// written into R2018 from R2007 up, which share the stream layout (see
    /// [`SpatialIndex::is_convertible`]).
    pub source_version: Version,
}

impl SpatialIndex {
    pub(crate) fn decode(r: &mut ObjReader) -> Result<Self> {
        let timestamps = [r.data.read_bl()?, r.data.read_bl()?];
        let bits = OpaqueBits::take(r)?;
        let (handles, handle_tail) = trailing_refs(r)?;
        Ok(SpatialIndex {
            timestamps,
            bits,
            handles,
            handle_tail,
            source_version: r.version,
        })
    }

    /// Whether the kept bits can be written as R2018 (read from R2007 up).
    pub fn is_convertible(&self) -> bool {
        self.source_version >= Version::R2007
    }

    pub(crate) fn encode(&self, w: &mut ObjWriter) -> Result<()> {
        if !self.is_convertible() {
            return Err(encode_err(
                "spatial index bits read before R2007 cannot be written as R2018",
            ));
        }
        w.data.write_bl(self.timestamps[0]);
        w.data.write_bl(self.timestamps[1]);
        self.bits.write(w)?;
        for &h in &self.handles {
            w.href(h)?;
        }
        self.handle_tail.write(w)
    }
}

/// A layer index entry: ODS's "Indexlong", the layer name and the index's
/// entry object.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct LayerIndexEntry {
    pub index: i32,
    pub name: String,
    pub handle: Handle,
}

/// A layer index (ODS 20.4.83, LAYER_INDEX). Decoded field by field rather
/// than kept opaque: its layer names are text, which moves to the string
/// stream from R2007, so stored bits could not be carried between versions.
#[derive(Debug, Clone, PartialEq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct LayerIndex {
    pub timestamps: [i32; 2],
    pub entries: Vec<LayerIndexEntry>,
}

impl LayerIndex {
    pub(crate) fn decode(r: &mut ObjReader) -> Result<Self> {
        let timestamps = [r.data.read_bl()?, r.data.read_bl()?];
        let n = read_count(r, MIN_BL, "layer index entries")?;
        check_handles(r, n, "layer index entries")?;
        let mut entries = Vec::with_capacity(n);
        for _ in 0..n {
            let index = r.data.read_bl()?;
            let name = r.text()?;
            entries.push(LayerIndexEntry {
                index,
                name,
                handle: Handle(0),
            });
        }
        for e in &mut entries {
            e.handle = r.href_abs()?;
        }
        Ok(LayerIndex {
            timestamps,
            entries,
        })
    }

    pub(crate) fn encode(&self, w: &mut ObjWriter) -> Result<()> {
        w.data.write_bl(self.timestamps[0]);
        w.data.write_bl(self.timestamps[1]);
        write_count(w, self.entries.len(), "layer index entries")?;
        for e in &self.entries {
            w.data.write_bl(e.index);
            w.text(&e.name)?;
        }
        for e in &self.entries {
            put(w, HARD_POINTER, e.handle)?;
        }
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::bits::BitWriter;

    use crate::error::Error;
    use crate::handle::{Handle, HandleRef};
    use crate::object::entities::tests::{reader, roundtrip};
    use crate::object::objects::test_util::H;
    use crate::object::objects::*;
    use crate::object::stream::tests::assemble;
    use crate::object::stream::{ObjReader, ObjWriter};

    use crate::version::Version;

    #[test]
    fn roundtrip_small_objects() {
        let i = ImageDef {
            class_version: 0,
            size: [994.0, 965.0],
            path: ".\\image ü.JPG".into(),
            loaded: true,
            resolution_units: 5,
            pixel_size: [0.26, 0.26],
        };
        roundtrip(&i, ImageDef::encode, ImageDef::decode, 500);
        let r = ImageDefReactor { class_version: 2 };
        roundtrip(&r, ImageDefReactor::encode, ImageDefReactor::decode, 500);
        let v = RasterVariables {
            class_version: 0,
            display_frame: 1,
            display_quality: 1,
            units: 5,
        };
        roundtrip(&v, RasterVariables::encode, RasterVariables::decode, 500);
        let v = WipeoutVariables { display_frame: 1 };
        roundtrip(&v, WipeoutVariables::encode, WipeoutVariables::decode, 500);
        let s = Scale {
            unknown: 0,
            name: "1:100 ü".into(),
            paper_units: 1.0,
            drawing_units: 10.0,
            unit_scale: true,
        };
        roundtrip(&s, Scale::encode, Scale::decode, 500);
        let s = SortEntsTable {
            block: Handle(0x1F),
            entries: vec![
                (Handle(0x35A), Handle(0x353)),
                (Handle(0x354), Handle(0x35A)),
            ],
        };
        roundtrip(&s, SortEntsTable::encode, SortEntsTable::decode, 500);
        let l = LayerIndex {
            timestamps: [2459632, 37383000],
            entries: vec![
                LayerIndexEntry {
                    index: 3,
                    name: "0".into(),
                    handle: Handle(0x93),
                },
                LayerIndexEntry {
                    index: 7,
                    name: "Wände".into(),
                    handle: Handle(0x95),
                },
            ],
        };
        roundtrip(&l, LayerIndex::encode, LayerIndex::decode, 500);
    }

    /// Sort handles are code 0 references in the data stream; their count is
    /// checked against both streams.
    #[test]
    fn sort_table_errors() {
        let mut d = BitWriter::new();
        d.write_bl(1);
        d.write_handle_ref(HandleRef {
            code: 5,
            value: 0x35A,
        })
        .unwrap();
        let mut h = BitWriter::new();
        for value in [0x1F, 0x353] {
            h.write_handle_ref(HandleRef { code: 4, value }).unwrap();
        }
        let bytes = assemble(Version::R2018, 500, &d, None, &h);
        assert!(matches!(
            SortEntsTable::decode(&mut reader(&bytes)),
            Err(Error::Corrupt(m)) if m.contains("code 5")
        ));
        let mut d = BitWriter::new();
        d.write_bl(1000);
        let bytes = assemble(Version::R2018, 500, &d, None, &h);
        assert!(SortEntsTable::decode(&mut reader(&bytes)).is_err());
    }

    /// Spatial filters and indexes keep their bits; a spatial index read
    /// before R2007 cannot be written as R2018, a filter (no text) can.
    #[test]
    fn opaque_objects() {
        let mut d = BitWriter::new();
        d.write_bs(2);
        d.write_2rd([1.0, 2.0]);
        d.write_2rd([3.0, 4.0]);
        d.write_3bd([0.0, 0.0, 1.0]);
        let mut h = BitWriter::new();
        h.write_handle_ref(HandleRef {
            code: 4,
            value: 0x77,
        })
        .unwrap();
        for v in [Version::R2000, Version::R2004, Version::R2018] {
            let bytes = assemble(v, 500, &d, None, &h);
            let (mut r, _) = ObjReader::split(&bytes, v, 30).unwrap();
            let f = SpatialFilter::decode(&mut r).unwrap();
            assert_eq!(f.bits.data_bits, d.bit_len(), "{v:?}");
            assert_eq!(f.handles.len(), 1);
            let mut w = ObjWriter::new(H);
            f.encode(&mut w).unwrap();
            let out = w.finish(500).unwrap();
            assert_eq!(SpatialFilter::decode(&mut reader(&out)).unwrap(), f);

            let (mut r, _) = ObjReader::split(&bytes, v, 30).unwrap();
            let s = SpatialIndex::decode(&mut r).unwrap();
            assert_eq!(s.is_convertible(), v >= Version::R2007, "{v:?}");
            let mut w = ObjWriter::new(H);
            assert_eq!(s.encode(&mut w).is_ok(), s.is_convertible());
            if s.is_convertible() {
                let out = w.finish(500).unwrap();
                assert_eq!(SpatialIndex::decode(&mut reader(&out)).unwrap(), s);
            }
        }
    }
}
