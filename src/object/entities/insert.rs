//! Block references (ODS 20.4.9 INSERT, 20.4.10 MINSERT).
//!
//! An insert with attributes owns them and the SEQEND that ends them. R2000
//! files store the first and last attribute (the others are found through
//! each entity's links, like a block record's entities); R2004+ files store
//! the count and every attribute. [`crate::Drawing`] turns the R2000 pair
//! into the list after reading (see [`EntityList`]).
//!
//! As ACadSharp (MIT, `DwgObjectReader.readInsertCommonData`) reads it, and
//! as the corpus confirms, the R2004+ owned-object count is stored only when
//! the insert has attributes (ODS lists it unconditionally).

use super::super::stream::{ObjReader, ObjWriter};
use super::super::tables::{
    EntityList, HARD_OWNER, HARD_POINTER, SOFT_POINTER, encode_count, handle_count, put,
};
use crate::error::{Error, Result};
use crate::handle::Handle;
use crate::version::Version;

/// A block reference (ODS 20.4.9).
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Insert {
    /// Insertion point.
    pub insertion: [f64; 3],
    /// X, Y and Z scale (DXF 41-43).
    pub scale: [f64; 3],
    /// Rotation, radians.
    pub rotation: f64,
    /// Normal (extrusion direction).
    pub extrusion: [f64; 3],
    /// The block record (hard pointer).
    pub block: Handle,
    /// The attributes (`Owned(vec![])` when there are none).
    pub attribs: EntityList,
    /// The SEQEND ending the attributes (hard owner); `Some` exactly when
    /// the insert has attributes (DXF 66).
    pub seqend: Option<Handle>,
}

impl Insert {
    /// An insert of block record `block` at `insertion`, scale 1,
    /// unrotated, in the XY plane, without attributes (add attributes with
    /// [`crate::Drawing::add_insert_with_attribs`]).
    pub fn new(block: Handle, insertion: [f64; 3]) -> Insert {
        Insert {
            insertion,
            scale: [1.0; 3],
            rotation: 0.0,
            extrusion: [0.0, 0.0, 1.0],
            block,
            attribs: EntityList::Owned(Vec::new()),
            seqend: None,
        }
    }
}

/// The scale data flags (ODS 20.4.9).
const SCALE_ONES: u8 = 3;
const SCALE_UNIFORM: u8 = 2;
const SCALE_X_ONE: u8 = 1;
const SCALE_FULL: u8 = 0;

fn same(a: f64, b: f64) -> bool {
    a.to_bits() == b.to_bits()
}

impl Insert {
    /// The data fields; the owned-object count is returned with them.
    fn decode_data(r: &mut ObjReader) -> Result<(Self, Option<i32>)> {
        let d = &mut r.data;
        let insertion = d.read_3bd()?;
        let scale = match d.read_bb()? {
            SCALE_ONES => [1.0; 3],
            SCALE_X_ONE => [1.0, d.read_dd(1.0)?, d.read_dd(1.0)?],
            SCALE_UNIFORM => [d.read_rd()?; 3],
            _ => {
                let x = d.read_rd()?;
                [x, d.read_dd(x)?, d.read_dd(x)?]
            }
        };
        let rotation = d.read_bd()?;
        let extrusion = d.read_3bd()?;
        let has_attribs = d.read_b()?;
        let owned = if has_attribs && r.version >= Version::R2004 {
            Some(r.data.read_bl()?)
        } else {
            None
        };
        let insert = Insert {
            insertion,
            scale,
            rotation,
            extrusion,
            block: Handle(0),
            attribs: EntityList::Owned(Vec::new()),
            seqend: has_attribs.then_some(Handle(0)),
        };
        Ok((insert, owned))
    }

    /// The handles, after the data fields (and an MINSERT's array fields).
    fn decode_handles(&mut self, r: &mut ObjReader, owned: Option<i32>) -> Result<()> {
        self.block = r.href_abs()?;
        if self.seqend.is_none() {
            return Ok(());
        }
        self.attribs = match owned {
            Some(n) => {
                let n = handle_count(r, n, "attribute")?;
                EntityList::Owned((0..n).map(|_| r.href_abs()).collect::<Result<_>>()?)
            }
            None => EntityList::Linked {
                first: r.href_abs()?,
                last: r.href_abs()?,
            },
        };
        self.seqend = Some(r.href_abs()?);
        Ok(())
    }

    /// The attribute handles to write, checked against `seqend`.
    fn attrib_list(&self) -> Result<&[Handle]> {
        match (&self.attribs, self.seqend) {
            (EntityList::Linked { .. }, _) => Err(Error::Encode(
                "an R2000 insert's first/last attributes must be converted to a list".into(),
            )),
            (EntityList::Owned(v), None) if !v.is_empty() => Err(Error::Encode(
                "an insert with attributes needs a SEQEND".into(),
            )),
            (EntityList::Owned(v), _) => Ok(v),
        }
    }

    fn encode_data(&self, w: &mut ObjWriter) -> Result<()> {
        let attribs = self.attrib_list()?;
        let d = &mut w.data;
        d.write_3bd(self.insertion);
        let [x, y, z] = self.scale;
        if [x, y, z].iter().all(|&s| same(s, 1.0)) {
            d.write_bb(SCALE_ONES);
        } else if same(x, 1.0) {
            d.write_bb(SCALE_X_ONE);
            d.write_dd(y, 1.0);
            d.write_dd(z, 1.0);
        } else if same(x, y) && same(x, z) {
            d.write_bb(SCALE_UNIFORM);
            d.write_rd(x);
        } else {
            d.write_bb(SCALE_FULL);
            d.write_rd(x);
            d.write_dd(y, x);
            d.write_dd(z, x);
        }
        d.write_bd(self.rotation);
        d.write_3bd(self.extrusion);
        d.write_b(self.seqend.is_some());
        if self.seqend.is_some() {
            d.write_bl(encode_count(attribs.len(), "attributes")?);
        }
        Ok(())
    }

    fn encode_handles(&self, w: &mut ObjWriter, attrib_code: u8) -> Result<()> {
        put(w, HARD_POINTER, self.block)?;
        if let Some(seqend) = self.seqend {
            for &h in self.attrib_list()? {
                put(w, attrib_code, h)?;
            }
            put(w, HARD_OWNER, seqend)?;
        }
        Ok(())
    }

    pub(crate) fn decode(r: &mut ObjReader) -> Result<Self> {
        let (mut insert, owned) = Insert::decode_data(r)?;
        insert.decode_handles(r, owned)?;
        Ok(insert)
    }

    pub(crate) fn encode(&self, w: &mut ObjWriter) -> Result<()> {
        self.encode_data(w)?;
        self.encode_handles(w, HARD_OWNER)
    }
}

/// An array of block references (ODS 20.4.10).
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct MInsert {
    pub insert: Insert,
    pub columns: i16,
    pub rows: i16,
    pub column_spacing: f64,
    pub row_spacing: f64,
}

impl MInsert {
    pub(crate) fn decode(r: &mut ObjReader) -> Result<Self> {
        let (mut insert, owned) = Insert::decode_data(r)?;
        let columns = r.data.read_bs()?;
        let rows = r.data.read_bs()?;
        let column_spacing = r.data.read_bd()?;
        let row_spacing = r.data.read_bd()?;
        insert.decode_handles(r, owned)?;
        Ok(MInsert {
            insert,
            columns,
            rows,
            column_spacing,
            row_spacing,
        })
    }

    /// ODS gives an MINSERT's R2004+ attributes as soft pointers (an
    /// INSERT's are hard owners); unconfirmed, as the corpus has no MINSERT.
    pub(crate) fn encode(&self, w: &mut ObjWriter) -> Result<()> {
        self.insert.encode_data(w)?;
        w.data.write_bs(self.columns);
        w.data.write_bs(self.rows);
        w.data.write_bd(self.column_spacing);
        w.data.write_bd(self.row_spacing);
        self.insert.encode_handles(w, SOFT_POINTER)
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::roundtrip;
    use super::*;
    use crate::bits::BitWriter;
    use crate::object::stream::tests::assemble;
    use crate::object::type_code;

    fn insert() -> Insert {
        Insert {
            insertion: [10.0, 20.0, 0.0],
            scale: [2.0, 3.0, 4.0],
            rotation: 0.5,
            extrusion: [0.0, 0.0, 1.0],
            block: Handle(0x1F0),
            attribs: EntityList::Owned(vec![Handle(0x201), Handle(0x202)]),
            seqend: Some(Handle(0x203)),
        }
    }

    #[test]
    fn roundtrip_insert() {
        let mut i = insert();
        roundtrip(&i, Insert::encode, Insert::decode, type_code::INSERT);
        // Every scale form, and no attributes.
        i.attribs = EntityList::Owned(vec![]);
        i.seqend = None;
        for scale in [[1.0; 3], [1.0, 2.0, 1.0], [2.5; 3], [2.0, 2.0, 1.0]] {
            i.scale = scale;
            roundtrip(&i, Insert::encode, Insert::decode, type_code::INSERT);
        }
        // Attributes may be empty while a SEQEND is stored.
        i.seqend = Some(Handle(0x203));
        roundtrip(&i, Insert::encode, Insert::decode, type_code::INSERT);
    }

    #[test]
    fn insert_scale_forms_are_the_shortest() {
        let len = |scale: [f64; 3]| {
            let mut w = ObjWriter::new(Handle(1));
            Insert { scale, ..insert() }.encode(&mut w).unwrap();
            w.data.bit_len()
        };
        let ones = len([1.0; 3]);
        assert_eq!(len([1.0, 1.0, 2.0]), ones + 2 + 64 + 2);
        assert_eq!(len([3.0; 3]), ones + 64);
        assert_eq!(len([3.0, 3.0, 2.0]), ones + 64 + 2 + 2 + 64);
    }

    #[test]
    fn roundtrip_minsert() {
        let m = MInsert {
            insert: insert(),
            columns: 3,
            rows: 2,
            column_spacing: 10.0,
            row_spacing: 5.5,
        };
        roundtrip(&m, MInsert::encode, MInsert::decode, type_code::MINSERT);
    }

    #[test]
    fn linked_or_unended_attribs_do_not_encode() {
        let mut i = insert();
        i.attribs = EntityList::Linked {
            first: Handle(0x201),
            last: Handle(0x202),
        };
        assert!(i.encode(&mut ObjWriter::new(Handle(1))).is_err());
        i.attribs = EntityList::Owned(vec![Handle(0x201)]);
        i.seqend = None;
        assert!(i.encode(&mut ObjWriter::new(Handle(1))).is_err());
    }

    /// An R2004 INSERT without attributes stores no owned-object count
    /// (ODS lists it unconditionally; ACadSharp and the corpus agree).
    #[test]
    fn r2004_insert_without_attribs_has_no_count() {
        let mut d = BitWriter::new();
        d.write_3bd([1.0, 2.0, 0.0]);
        d.write_bb(SCALE_UNIFORM);
        d.write_rd(2.5);
        d.write_bd(0.5);
        d.write_3bd([0.0, 0.0, 1.0]);
        d.write_b(false);
        let mut h = BitWriter::new();
        h.write_handle_ref(crate::handle::HandleRef {
            code: 5,
            value: 0x1F0,
        })
        .unwrap();
        let bytes = assemble(Version::R2004, type_code::INSERT, &d, None, &h);
        let (mut r, _) = ObjReader::split(&bytes, Version::R2004, 30).unwrap();
        let i = Insert::decode(&mut r).unwrap();
        assert_eq!(r.data.remaining_bits(), 0);
        assert_eq!(
            (i.scale, i.rotation, i.block),
            ([2.5; 3], 0.5, Handle(0x1F0))
        );
        assert_eq!((i.seqend, i.attribs), (None, EntityList::Owned(vec![])));
    }

    /// An R2000 INSERT: no owned count; the first and last attribute.
    #[test]
    fn r2000_insert_attribs_are_linked() {
        let mut d = BitWriter::new();
        d.write_3bd([1.0, 2.0, 0.0]);
        d.write_bb(SCALE_ONES);
        d.write_bd(0.0);
        d.write_3bd([0.0, 0.0, 1.0]);
        d.write_b(true);
        let mut h = BitWriter::new();
        for v in [0x1F0, 0x201, 0x205, 0x206] {
            h.write_handle_ref(crate::handle::HandleRef { code: 4, value: v })
                .unwrap();
        }
        let bytes = assemble(Version::R2000, type_code::INSERT, &d, None, &h);
        let (mut r, _) = ObjReader::split(&bytes, Version::R2000, 30).unwrap();
        let i = Insert::decode(&mut r).unwrap();
        assert_eq!(i.block, Handle(0x1F0));
        assert_eq!(
            i.attribs,
            EntityList::Linked {
                first: Handle(0x201),
                last: Handle(0x205)
            }
        );
        assert_eq!(i.seqend, Some(Handle(0x206)));
    }
}
