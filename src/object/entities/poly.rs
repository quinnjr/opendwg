//! Polylines and curves (ODS 20.4.11-20.4.17, 20.4.33, 20.4.34, 20.4.40,
//! 20.4.50, 20.4.85): lightweight polylines; 2D, 3D, polyface mesh and
//! polygon mesh polylines with their vertices; splines; and multilines.
//!
//! A POLYLINE owns its VERTEX entities and the SEQEND that ends them, as an
//! INSERT owns its attributes: R2000 files store the first and last vertex,
//! R2004+ files the count and every vertex. [`crate::Drawing`] turns the
//! R2000 pair into the list after reading (see [`EntityList`]).
//!
//! Where the corpus or ACadSharp (MIT) differ from ODS:
//! - R2004+ polyface mesh vertices are hard owners (ODS: soft pointers), as
//!   the corpus shows. Polygon mesh vertices, not in the corpus, are written
//!   as hard owners too, as ACadSharp (`DwgObjectWriter.writePolygonMesh`)
//!   writes them.
//! - LWPOLYLINE flag bits 0x100 and 0x200 are "plinegen" and "closed"
//!   (ACadSharp `DwgObjectReader.readLWPolyline`; the corpus agrees: DXF 70 =
//!   1 where 0x200 is set).
//! - An R2013+ SPLINE stores fit data exactly when its knot parameter is not
//!   "custom" and flags 1 has "use knot parameter" (ACadSharp
//!   `DwgObjectReader.readSpline`); its scenario field does not decide (the
//!   corpus has fit data with scenario 1).
//! - The MLINE "open/closed" short is DXF 71, the flags (1 has vertices, 2
//!   closed, 4 and 8 caps suppressed); the corpus has 1 and 5.

use super::super::common::AsRead;
use super::super::stream::{ObjReader, ObjWriter};
use super::super::tables::{EntityList, HARD_OWNER, HARD_POINTER, encode_count, handle_count, put};
use crate::bits::BitReader;
use crate::error::encode_error as encode_err;
use crate::error::{Error, Result};
use crate::handle::Handle;
use crate::version::Version;

fn corrupt(msg: impl Into<String>) -> Error {
    Error::Corrupt(msg.into())
}

/// Whether `a` and `b` are the same value, bit for bit.
fn same(a: f64, b: f64) -> bool {
    a.to_bits() == b.to_bits()
}

/// Whether `v` is +0.0 (a stored -0.0 is kept by writing it).
fn is_zero(v: f64) -> bool {
    v.to_bits() == 0
}

const UNIT_Z: [f64; 3] = [0.0, 0.0, 1.0];

/// A count read from the data stream, which must not be negative.
fn count(n: i32, what: &str) -> Result<usize> {
    usize::try_from(n).map_err(|_| corrupt(format!("negative {what} count {n}")))
}

/// Check that `(count, minimum bits per item)` lists fit in the data stream
/// left, before allocating for them.
fn check_fits(r: &ObjReader, items: &[(usize, u64)], context: &'static str) -> Result<()> {
    let need = items.iter().try_fold(0u64, |acc, &(n, bits)| {
        (n as u64)
            .checked_mul(bits)
            .and_then(|b| acc.checked_add(b))
    });
    match need {
        Some(n) if n <= r.data.remaining_bits() => Ok(()),
        _ => Err(Error::Truncated { context }),
    }
}

/// The smallest encodings: a BD or BL is 2 bits, a 3BD 6, a 2RD 128.
const MIN_BD: u64 = 2;
const MIN_BL: u64 = 2;
const MIN_3BD: u64 = 6;
const MIN_BS: u64 = 2;

/// An `i16` count to write.
fn short_count(n: usize, what: &str) -> Result<i16> {
    i16::try_from(n).map_err(|_| encode_err(format!("too many {what} to encode")))
}

// ---------------------------------------------------------------------------
// Polylines and their vertices.

/// R2004+: the owned-object count, stored after the type fields.
fn read_owned(r: &mut ObjReader) -> Result<Option<i32>> {
    Ok(if r.version >= Version::R2004 {
        Some(r.data.read_bl()?)
    } else {
        None
    })
}

/// The vertex handles and the SEQEND, after the common entity handles.
fn read_vertices(r: &mut ObjReader, owned: Option<i32>) -> Result<(EntityList, Handle)> {
    let vertices = match owned {
        Some(n) => {
            let n = handle_count(r, n, "vertex")?;
            EntityList::Owned((0..n).map(|_| r.href_abs()).collect::<Result<_>>()?)
        }
        None => EntityList::Linked {
            first: r.href_abs()?,
            last: r.href_abs()?,
        },
    };
    Ok((vertices, r.href_abs()?))
}

/// The vertex handles to write.
fn owned_vertices(list: &EntityList) -> Result<&[Handle]> {
    match list {
        EntityList::Linked { .. } => Err(encode_err(
            "an R2000 polyline's first/last vertices must be converted to a list",
        )),
        EntityList::Owned(v) => Ok(v),
    }
}

fn write_owned(w: &mut ObjWriter, list: &EntityList) -> Result<()> {
    let n = encode_count(owned_vertices(list)?.len(), "vertices")?;
    w.data.write_bl(n);
    Ok(())
}

fn write_vertices(w: &mut ObjWriter, list: &EntityList, seqend: Handle) -> Result<()> {
    for &h in owned_vertices(list)? {
        put(w, HARD_OWNER, h)?;
    }
    put(w, HARD_OWNER, seqend)
}

/// A 2D polyline (ODS 20.4.16), with VERTEX_2D vertices.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct Polyline2d {
    /// DXF 70.
    pub flags: i16,
    /// Curve and smooth surface type (DXF 75).
    pub curve_type: i16,
    pub start_width: f64,
    pub end_width: f64,
    pub thickness: f64,
    /// The OCS Z of every vertex (the 10 point is (0, 0, elevation)).
    pub elevation: f64,
    pub extrusion: [f64; 3],
    /// The vertices (`Owned` once read; see [`EntityList`]).
    pub vertices: EntityList,
    /// The SEQEND ending the vertices (hard owner).
    pub seqend: Handle,
}

impl Polyline2d {
    pub(crate) fn decode(r: &mut ObjReader) -> Result<Self> {
        let d = &mut r.data;
        let flags = d.read_bs()?;
        let curve_type = d.read_bs()?;
        let start_width = d.read_bd()?;
        let end_width = d.read_bd()?;
        let thickness = d.read_bt()?;
        let elevation = d.read_bd()?;
        let extrusion = d.read_be()?;
        let owned = read_owned(r)?;
        let (vertices, seqend) = read_vertices(r, owned)?;
        Ok(Polyline2d {
            flags,
            curve_type,
            start_width,
            end_width,
            thickness,
            elevation,
            extrusion,
            vertices,
            seqend,
        })
    }

    pub(crate) fn encode(&self, w: &mut ObjWriter) -> Result<()> {
        let d = &mut w.data;
        d.write_bs(self.flags);
        d.write_bs(self.curve_type);
        d.write_bd(self.start_width);
        d.write_bd(self.end_width);
        d.write_bt(self.thickness);
        d.write_bd(self.elevation);
        d.write_be(self.extrusion);
        write_owned(w, &self.vertices)?;
        write_vertices(w, &self.vertices, self.seqend)
    }
}

/// A 3D polyline (ODS 20.4.17), with VERTEX_3D vertices.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct Polyline3d {
    /// Bit 0: splined, quadratic (DXF 75 = 5); bit 1: splined, cubic (75 =
    /// 6). Either sets DXF 70 bit 4.
    pub spline_flags: u8,
    /// Bit 0: closed (DXF 70 bit 1). DXF 70 bit 8 (3D) is implied.
    pub closed_flags: u8,
    pub vertices: EntityList,
    pub seqend: Handle,
}

impl Polyline3d {
    pub(crate) fn decode(r: &mut ObjReader) -> Result<Self> {
        let spline_flags = r.data.read_rc()?;
        let closed_flags = r.data.read_rc()?;
        let owned = read_owned(r)?;
        let (vertices, seqend) = read_vertices(r, owned)?;
        Ok(Polyline3d {
            spline_flags,
            closed_flags,
            vertices,
            seqend,
        })
    }

    pub(crate) fn encode(&self, w: &mut ObjWriter) -> Result<()> {
        w.data.write_rc(self.spline_flags);
        w.data.write_rc(self.closed_flags);
        write_owned(w, &self.vertices)?;
        write_vertices(w, &self.vertices, self.seqend)
    }
}

/// A polyface mesh (ODS 20.4.33): VERTEX_PFACE locations followed by
/// VERTEX_PFACE_FACE faces, all in `vertices`.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct PolyfaceMesh {
    /// The number of locations (DXF 71).
    pub vertex_count: i16,
    /// The number of faces (DXF 72).
    pub face_count: i16,
    pub vertices: EntityList,
    pub seqend: Handle,
}

impl PolyfaceMesh {
    pub(crate) fn decode(r: &mut ObjReader) -> Result<Self> {
        let vertex_count = r.data.read_bs()?;
        let face_count = r.data.read_bs()?;
        let owned = read_owned(r)?;
        let (vertices, seqend) = read_vertices(r, owned)?;
        Ok(PolyfaceMesh {
            vertex_count,
            face_count,
            vertices,
            seqend,
        })
    }

    pub(crate) fn encode(&self, w: &mut ObjWriter) -> Result<()> {
        w.data.write_bs(self.vertex_count);
        w.data.write_bs(self.face_count);
        write_owned(w, &self.vertices)?;
        write_vertices(w, &self.vertices, self.seqend)
    }
}

/// A polygon mesh (ODS 20.4.34), with VERTEX_MESH vertices, M rows of N.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct PolygonMesh {
    /// DXF 70.
    pub flags: i16,
    /// Curve and smooth surface type (DXF 75).
    pub curve_type: i16,
    /// DXF 71 and 72.
    pub m_count: i16,
    pub n_count: i16,
    /// Smooth surface densities (DXF 73 and 74).
    pub m_density: i16,
    pub n_density: i16,
    pub vertices: EntityList,
    pub seqend: Handle,
}

impl PolygonMesh {
    pub(crate) fn decode(r: &mut ObjReader) -> Result<Self> {
        let d = &mut r.data;
        let flags = d.read_bs()?;
        let curve_type = d.read_bs()?;
        let m_count = d.read_bs()?;
        let n_count = d.read_bs()?;
        let m_density = d.read_bs()?;
        let n_density = d.read_bs()?;
        let owned = read_owned(r)?;
        let (vertices, seqend) = read_vertices(r, owned)?;
        Ok(PolygonMesh {
            flags,
            curve_type,
            m_count,
            n_count,
            m_density,
            n_density,
            vertices,
            seqend,
        })
    }

    pub(crate) fn encode(&self, w: &mut ObjWriter) -> Result<()> {
        let d = &mut w.data;
        for v in [
            self.flags,
            self.curve_type,
            self.m_count,
            self.n_count,
            self.m_density,
            self.n_density,
        ] {
            d.write_bs(v);
        }
        write_owned(w, &self.vertices)?;
        write_vertices(w, &self.vertices, self.seqend)
    }
}

/// A 2D polyline vertex (ODS 20.4.11). The point is in OCS; its Z is
/// normally 0 (the polyline's elevation applies).
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct Vertex2d {
    /// DXF 70.
    pub flags: u8,
    pub point: [f64; 3],
    pub start_width: f64,
    pub end_width: f64,
    pub bulge: f64,
    /// R2010+ (DXF 91); 0 before.
    pub id: i32,
    /// Curve fit tangent direction (DXF 50).
    pub tangent_dir: f64,
}

impl Vertex2d {
    pub(crate) fn decode(r: &mut ObjReader) -> Result<Self> {
        let d = &mut r.data;
        let flags = d.read_rc()?;
        let point = d.read_3bd()?;
        // A negative start width stands for equal start and end widths.
        let start = d.read_bd()?;
        let (start_width, end_width) = if start < 0.0 {
            (-start, -start)
        } else {
            (start, d.read_bd()?)
        };
        let bulge = d.read_bd()?;
        let id = if r.version >= Version::R2010 {
            r.data.read_bl()?
        } else {
            0
        };
        Ok(Vertex2d {
            flags,
            point,
            start_width,
            end_width,
            bulge,
            id,
            tangent_dir: r.data.read_bd()?,
        })
    }

    /// Equal positive widths are written once, negated, as ODS describes. A
    /// negative start width cannot be stored.
    pub(crate) fn encode(&self, w: &mut ObjWriter) -> Result<()> {
        if self.start_width < 0.0 {
            return Err(encode_err("a 2D vertex start width cannot be negative"));
        }
        let d = &mut w.data;
        d.write_rc(self.flags);
        d.write_3bd(self.point);
        if self.start_width > 0.0 && same(self.start_width, self.end_width) {
            d.write_bd(-self.start_width);
        } else {
            d.write_bd(self.start_width);
            d.write_bd(self.end_width);
        }
        d.write_bd(self.bulge);
        d.write_bl(self.id);
        d.write_bd(self.tangent_dir);
        Ok(())
    }
}

/// A 3D polyline vertex (ODS 20.4.12); also a polygon mesh vertex (20.4.13)
/// and a polyface mesh location (20.4.14), which have the same layout.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct Vertex3d {
    /// DXF 70: 32 for a 3D polyline vertex, 64 for a mesh vertex, 192 for
    /// a polyface location.
    pub flags: u8,
    pub point: [f64; 3],
}

/// A polygon mesh vertex.
pub type VertexMesh = Vertex3d;
/// A polyface mesh location.
pub type VertexPface = Vertex3d;

impl Vertex3d {
    pub(crate) fn decode(r: &mut ObjReader) -> Result<Self> {
        Ok(Vertex3d {
            flags: r.data.read_rc()?,
            point: r.data.read_3bd()?,
        })
    }

    pub(crate) fn encode(&self, w: &mut ObjWriter) -> Result<()> {
        w.data.write_rc(self.flags);
        w.data.write_3bd(self.point);
        Ok(())
    }
}

/// A polyface mesh face (ODS 20.4.15).
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct VertexPfaceFace {
    /// 1-based location indices (DXF 71-74); a negative index hides the
    /// edge that starts there, 0 ends a triangle.
    pub indices: [i16; 4],
}

impl VertexPfaceFace {
    pub(crate) fn decode(r: &mut ObjReader) -> Result<Self> {
        let mut indices = [0; 4];
        for i in &mut indices {
            *i = r.data.read_bs()?;
        }
        Ok(VertexPfaceFace { indices })
    }

    pub(crate) fn encode(&self, w: &mut ObjWriter) -> Result<()> {
        for i in self.indices {
            w.data.write_bs(i);
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Lightweight polylines.

/// LWPOLYLINE flag bit: linetype generated continuously through the
/// vertices (DXF 70 bit 128).
pub const LWPOLYLINE_PLINEGEN: i16 = 0x100;
/// LWPOLYLINE flag bit: closed (DXF 70 bit 1).
pub const LWPOLYLINE_CLOSED: i16 = 0x200;

/// The flag bits that say which fields are stored (ODS 20.4.85).
const LW_NORMAL: i16 = 0x1;
const LW_THICKNESS: i16 = 0x2;
const LW_CONST_WIDTH: i16 = 0x4;
const LW_ELEVATION: i16 = 0x8;
const LW_BULGES: i16 = 0x10;
const LW_WIDTHS: i16 = 0x20;
const LW_IDS: i16 = 0x400;
const LW_STORED: i16 =
    LW_NORMAL | LW_THICKNESS | LW_CONST_WIDTH | LW_ELEVATION | LW_BULGES | LW_WIDTHS | LW_IDS;

/// A lightweight polyline (ODS 20.4.85). Points are 2D, in OCS at
/// `elevation`.
///
/// The stored flags word also says which optional fields follow; those bits
/// are not kept but follow the fields when written: a scalar is stored
/// when it is not its default (+0.0, or (0, 0, 1) for the extrusion; a
/// stored -0.0 stays stored), and a list when it is not empty. Adding
/// bulges or widths therefore stores them.
///
/// A stored list has one entry per point (ODS; AutoCAD writes them so).
/// While the number of points is the one read, the lists are written as
/// they are; otherwise (points added or removed, or a polyline built by
/// hand) each non-empty list is written with `points.len()` entries,
/// truncated or padded with 0 bulges, 0 ids and (0, 0) widths.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct LwPolyline {
    /// The flags without the stored-field bits: [`LWPOLYLINE_CLOSED`],
    /// [`LWPOLYLINE_PLINEGEN`] and any other bits as read.
    pub flags: i16,
    /// DXF 43.
    pub const_width: f64,
    pub elevation: f64,
    pub thickness: f64,
    pub extrusion: [f64; 3],
    pub points: Vec<[f64; 2]>,
    /// DXF 42; empty when none are stored.
    pub bulges: Vec<f64>,
    /// R2010+ vertex identifiers (DXF 91); empty when none are stored.
    pub vertex_ids: Vec<i32>,
    /// Start and end widths (DXF 40, 41); empty when none are stored.
    pub widths: Vec<[f64; 2]>,
    /// The number of points read, which the stored list lengths go with.
    #[cfg_attr(feature = "serde", serde(skip))]
    pub read_points: AsRead<usize>,
}

/// `v` cut or padded with `pad` to `n` entries.
fn fitted<T: Copy>(v: &[T], n: usize, pad: T) -> impl Iterator<Item = T> + '_ {
    v.iter().copied().chain(std::iter::repeat(pad)).take(n)
}

impl LwPolyline {
    /// Whether the polyline is closed ([`LWPOLYLINE_CLOSED`]).
    pub fn closed(&self) -> bool {
        self.flags & LWPOLYLINE_CLOSED != 0
    }

    /// Whether the linetype pattern runs continuously around the vertices
    /// ([`LWPOLYLINE_PLINEGEN`]).
    pub fn plinegen(&self) -> bool {
        self.flags & LWPOLYLINE_PLINEGEN != 0
    }

    /// The number of entries to write for a list of `len`: none for an empty
    /// list; `len` while the point count is the one read; else one per
    /// point.
    fn list_len(&self, len: usize) -> usize {
        if len == 0 {
            0
        } else if self.read_points.get() == Some(&self.points.len()) {
            len
        } else {
            self.points.len()
        }
    }

    /// The bulge, vertex id and width counts to write.
    fn list_lens(&self) -> [usize; 3] {
        [
            self.list_len(self.bulges.len()),
            self.list_len(self.vertex_ids.len()),
            self.list_len(self.widths.len()),
        ]
    }

    /// The flags word to write: `flags` with the bits of the stored fields.
    fn stored_flags(&self) -> i16 {
        let mut f = self.flags & !LW_STORED;
        let [nb, ni, nw] = self.list_lens();
        for (stored, bit) in [
            (!is_zero(self.const_width), LW_CONST_WIDTH),
            (!is_zero(self.elevation), LW_ELEVATION),
            (!is_zero(self.thickness), LW_THICKNESS),
            (
                !self.extrusion.iter().zip(UNIT_Z).all(|(&a, b)| same(a, b)),
                LW_NORMAL,
            ),
            (nb > 0, LW_BULGES),
            (ni > 0, LW_IDS),
            (nw > 0, LW_WIDTHS),
        ] {
            if stored {
                f |= bit;
            }
        }
        f
    }

    /// Decode LWPOLYLINE data with no entity common data around it, as proxy
    /// graphics embed it (ODS chapter 29, opcode 33): from the flags `BS`
    /// to the width array, read as in an entity of `version`.
    pub(crate) fn decode_embedded(bytes: &[u8], version: Version) -> Result<Self> {
        let mut r = ObjReader::from_streams(
            BitReader::new(bytes),
            None,
            BitReader::new(&[]),
            version,
            30,
        );
        Self::decode(&mut r)
    }

    pub(crate) fn decode(r: &mut ObjReader) -> Result<Self> {
        let d = &mut r.data;
        let word = d.read_bs()?;
        let mut opt =
            |bit: i16| -> Result<f64> { Ok(if word & bit != 0 { d.read_bd()? } else { 0.0 }) };
        let const_width = opt(LW_CONST_WIDTH)?;
        let elevation = opt(LW_ELEVATION)?;
        let thickness = opt(LW_THICKNESS)?;
        let extrusion = if word & LW_NORMAL != 0 {
            d.read_3bd()?
        } else {
            UNIT_Z
        };
        let n = count(d.read_bl()?, "point")?;
        let mut list_count = |bit: i16, what: &str| -> Result<usize> {
            if word & bit != 0 {
                count(d.read_bl()?, what)
            } else {
                Ok(0)
            }
        };
        let nb = list_count(LW_BULGES, "bulge")?;
        // ODS: vertex identifiers from R2010.
        let ni = if r.version >= Version::R2010 {
            list_count(LW_IDS, "vertex id")?
        } else {
            0
        };
        let nw = list_count(LW_WIDTHS, "width")?;
        // The first point is 2RD; the others are DD pairs, 2 bits at least.
        check_fits(
            r,
            &[
                (usize::from(n > 0), 128),
                (n.saturating_sub(1), 2 * MIN_BD),
                (nb, MIN_BD),
                (ni, MIN_BL),
                (nw, 2 * MIN_BD),
            ],
            "lwpolyline lists",
        )?;
        let d = &mut r.data;
        let mut points = Vec::with_capacity(n);
        if n > 0 {
            let mut p = d.read_2rd()?;
            points.push(p);
            for _ in 1..n {
                p = d.read_2dd(p)?;
                points.push(p);
            }
        }
        let bulges = (0..nb).map(|_| d.read_bd()).collect::<Result<_>>()?;
        let vertex_ids = (0..ni).map(|_| d.read_bl()).collect::<Result<_>>()?;
        let widths = (0..nw)
            .map(|_| Ok([d.read_bd()?, d.read_bd()?]))
            .collect::<Result<_>>()?;
        Ok(LwPolyline {
            flags: word & !LW_STORED,
            const_width,
            elevation,
            thickness,
            extrusion,
            points,
            bulges,
            vertex_ids,
            widths,
            read_points: AsRead::new(n),
        })
    }

    pub(crate) fn encode(&self, w: &mut ObjWriter) -> Result<()> {
        let word = self.stored_flags();
        let [nb, ni, nw] = self.list_lens();
        let d = &mut w.data;
        d.write_bs(word);
        if word & LW_CONST_WIDTH != 0 {
            d.write_bd(self.const_width);
        }
        if word & LW_ELEVATION != 0 {
            d.write_bd(self.elevation);
        }
        if word & LW_THICKNESS != 0 {
            d.write_bd(self.thickness);
        }
        if word & LW_NORMAL != 0 {
            d.write_3bd(self.extrusion);
        }
        d.write_bl(encode_count(self.points.len(), "points")?);
        for n in [nb, ni, nw] {
            if n > 0 {
                d.write_bl(encode_count(n, "lwpolyline list entries")?);
            }
        }
        let mut prev = None;
        for &p in &self.points {
            match prev {
                None => d.write_2rd(p),
                Some(prev) => d.write_2dd(p, prev),
            }
            prev = Some(p);
        }
        for b in fitted(&self.bulges, nb, 0.0) {
            d.write_bd(b);
        }
        for id in fitted(&self.vertex_ids, ni, 0) {
            d.write_bl(id);
        }
        for [s, e] in fitted(&self.widths, nw, [0.0; 2]) {
            d.write_bd(s);
            d.write_bd(e);
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Splines.

/// SPLINE flags 1 (R2013+): defined by fit points.
pub const SPLINE_FLAGS1_METHOD_FIT: i32 = 1;
/// SPLINE flags 1: closed.
pub const SPLINE_FLAGS1_CLOSED: i32 = 4;
/// SPLINE flags 1: use the knot parameter.
pub const SPLINE_FLAGS1_USE_KNOT_PARAMETER: i32 = 8;
/// SPLINE knot parameters (R2013+): chord length, the default for fit data.
pub const SPLINE_KNOT_CHORD: i32 = 0;
/// SPLINE knot parameters: custom, which control points have.
pub const SPLINE_KNOT_CUSTOM: i32 = 15;

/// The scenario values ODS gives: control points, fit data.
const SCENARIO_CONTROL: i32 = 1;
const SCENARIO_FIT: i32 = 2;

/// A spline (ODS 20.4.40), defined by control points or by fit data, as
/// read.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct Spline {
    pub degree: i32,
    /// R2013+ flags 1 ([`SPLINE_FLAGS1_METHOD_FIT`], CV frame shown 2,
    /// [`SPLINE_FLAGS1_CLOSED`], [`SPLINE_FLAGS1_USE_KNOT_PARAMETER`]). Read
    /// from older files: closed for closed control points, fit points and
    /// knot parameter for fit data.
    pub flags1: i32,
    /// R2013+ knot parameterization: [`SPLINE_KNOT_CHORD`], square root 1,
    /// uniform 2, [`SPLINE_KNOT_CUSTOM`]. Read from older files: custom for
    /// control points, chord for fit data.
    pub knot_parameter: i32,
    pub form: SplineForm,
    /// What the stored copies were read with (see [`Spline::scenario`] and
    /// [`Spline::r2013_fields`]).
    #[cfg_attr(feature = "serde", serde(skip))]
    pub read: AsRead<SplineAsRead>,
}

/// A spline's form and the values stored with it, as read.
#[derive(Debug, Clone, Copy)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SplineAsRead {
    fit: bool,
    scenario: i32,
    closed: bool,
    rational: bool,
    weighted: bool,
}

/// How a spline is defined.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub enum SplineForm {
    Control(SplineControl),
    Fit(SplineFit),
}

impl SplineForm {
    fn is_fit(&self) -> bool {
        matches!(self, SplineForm::Fit(_))
    }
}

/// A spline defined by knots and control points.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct SplineControl {
    /// Whether the spline is rational (DXF 70 bit 4). Weights are stored
    /// exactly when `weights` is `Some`, and `rational` is written to agree
    /// with that, unless it was read disagreeing and neither has changed
    /// since (see [`Spline::rational_to_write`]).
    pub rational: bool,
    pub closed: bool,
    pub periodic: bool,
    /// DXF 42 and 43.
    pub knot_tol: f64,
    pub ctrl_tol: f64,
    pub knots: Vec<f64>,
    pub points: Vec<[f64; 3]>,
    /// One weight per control point (DXF 41) when stored. Each weight is a
    /// BD: ODS gives "D", ACadSharp (`readSpline`) reads a BD; the corpus has
    /// no weighted spline.
    pub weights: Option<Vec<f64>>,
}

/// A spline defined by fit points.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct SplineFit {
    /// DXF 44.
    pub fit_tol: f64,
    /// DXF 12 and 13.
    pub begin_tangent: [f64; 3],
    pub end_tangent: [f64; 3],
    pub points: Vec<[f64; 3]>,
}

impl Spline {
    /// The scenario field as read, or `None` for a spline built by hand.
    /// R2013+ files may store 1 for fit data.
    pub fn stored_scenario(&self) -> Option<i32> {
        self.read.get().map(|s| s.scenario)
    }

    /// The scenario field to write: as read while the form is the one read,
    /// otherwise ODS's value for the form (1 control points, 2 fit data).
    pub fn scenario(&self) -> i32 {
        match self.read.get() {
            Some(s) if s.fit == self.form.is_fit() => s.scenario,
            _ if self.form.is_fit() => SCENARIO_FIT,
            _ => SCENARIO_CONTROL,
        }
    }

    /// The R2013+ flags 1 and knot parameter to write. They decide whether
    /// fit data or control points are stored, so they are made to agree
    /// with the form: fit data gets fit points and the knot parameter bits
    /// (and chord length for a custom knot parameter), control points lose
    /// them and get a custom knot parameter. The closed bit of control
    /// points copies `closed`: as read while `closed` is unchanged.
    pub fn r2013_fields(&self) -> (i32, i32) {
        const LAYOUT_BITS: i32 = SPLINE_FLAGS1_METHOD_FIT | SPLINE_FLAGS1_USE_KNOT_PARAMETER;
        let (mut flags, mut knot) = (self.flags1, self.knot_parameter);
        let fit_layout =
            knot != SPLINE_KNOT_CUSTOM && flags & SPLINE_FLAGS1_USE_KNOT_PARAMETER != 0;
        match &self.form {
            SplineForm::Fit(_) => {
                if !fit_layout {
                    flags |= LAYOUT_BITS;
                    if knot == SPLINE_KNOT_CUSTOM {
                        knot = SPLINE_KNOT_CHORD;
                    }
                }
            }
            SplineForm::Control(c) => {
                if fit_layout {
                    flags &= !LAYOUT_BITS;
                    knot = SPLINE_KNOT_CUSTOM;
                }
                let read_closed = self.read.get().filter(|s| !s.fit).map(|s| s.closed);
                if read_closed != Some(c.closed) {
                    flags = if c.closed {
                        flags | SPLINE_FLAGS1_CLOSED
                    } else {
                        flags & !SPLINE_FLAGS1_CLOSED
                    };
                }
            }
        }
        (flags, knot)
    }

    /// The rational flag to write for control points `c`: as read while it
    /// and whether weights are stored are unchanged, otherwise whether
    /// weights are stored.
    pub fn rational_to_write(&self, c: &SplineControl) -> bool {
        match self.read.get() {
            Some(s) if !s.fit && s.rational == c.rational && s.weighted == c.weights.is_some() => {
                c.rational
            }
            _ => c.weights.is_some(),
        }
    }

    pub(crate) fn decode(r: &mut ObjReader) -> Result<Self> {
        let d = &mut r.data;
        let scenario = d.read_bl()?;
        let (mut flags1, knot_parameter, fit) = if r.version >= Version::R2013 {
            let flags = d.read_bl()?;
            let knot = d.read_bl()?;
            let fit = knot != SPLINE_KNOT_CUSTOM && flags & SPLINE_FLAGS1_USE_KNOT_PARAMETER != 0;
            (flags, knot, fit)
        } else {
            match scenario {
                SCENARIO_CONTROL => (0, SPLINE_KNOT_CUSTOM, false),
                SCENARIO_FIT => (
                    SPLINE_FLAGS1_METHOD_FIT | SPLINE_FLAGS1_USE_KNOT_PARAMETER,
                    SPLINE_KNOT_CHORD,
                    true,
                ),
                s => return Err(corrupt(format!("spline scenario {s}"))),
            }
        };
        let degree = d.read_bl()?;
        let form = if fit {
            let fit_tol = d.read_bd()?;
            let begin_tangent = d.read_3bd()?;
            let end_tangent = d.read_3bd()?;
            let n = count(d.read_bl()?, "fit point")?;
            check_fits(r, &[(n, MIN_3BD)], "spline fit points")?;
            let points = (0..n).map(|_| r.data.read_3bd()).collect::<Result<_>>()?;
            SplineForm::Fit(SplineFit {
                fit_tol,
                begin_tangent,
                end_tangent,
                points,
            })
        } else {
            let rational = d.read_b()?;
            let closed = d.read_b()?;
            let periodic = d.read_b()?;
            let knot_tol = d.read_bd()?;
            let ctrl_tol = d.read_bd()?;
            let nk = count(d.read_bl()?, "knot")?;
            let nc = count(d.read_bl()?, "control point")?;
            let weighted = d.read_b()?;
            let point_bits = MIN_3BD + if weighted { MIN_BD } else { 0 };
            check_fits(r, &[(nk, MIN_BD), (nc, point_bits)], "spline knots")?;
            let d = &mut r.data;
            let knots = (0..nk).map(|_| d.read_bd()).collect::<Result<_>>()?;
            let mut points = Vec::with_capacity(nc);
            let mut weights = weighted.then(|| Vec::with_capacity(nc));
            for _ in 0..nc {
                points.push(d.read_3bd()?);
                if let Some(w) = &mut weights {
                    w.push(d.read_bd()?);
                }
            }
            if r.version < Version::R2013 && closed {
                flags1 |= SPLINE_FLAGS1_CLOSED;
            }
            SplineForm::Control(SplineControl {
                rational,
                closed,
                periodic,
                knot_tol,
                ctrl_tol,
                knots,
                points,
                weights,
            })
        };
        let (closed, rational, weighted) = match &form {
            SplineForm::Control(c) => (c.closed, c.rational, c.weights.is_some()),
            SplineForm::Fit(_) => (false, false, false),
        };
        Ok(Spline {
            degree,
            flags1,
            knot_parameter,
            form,
            read: AsRead::new(SplineAsRead {
                fit,
                scenario,
                closed,
                rational,
                weighted,
            }),
        })
    }

    pub(crate) fn encode(&self, w: &mut ObjWriter) -> Result<()> {
        let (flags1, knot) = self.r2013_fields();
        let d = &mut w.data;
        d.write_bl(self.scenario());
        d.write_bl(flags1);
        d.write_bl(knot);
        d.write_bl(self.degree);
        match &self.form {
            SplineForm::Fit(f) => {
                d.write_bd(f.fit_tol);
                d.write_3bd(f.begin_tangent);
                d.write_3bd(f.end_tangent);
                d.write_bl(encode_count(f.points.len(), "fit points")?);
                for &p in &f.points {
                    d.write_3bd(p);
                }
            }
            SplineForm::Control(c) => {
                if c.weights
                    .as_ref()
                    .is_some_and(|w| w.len() != c.points.len())
                {
                    return Err(encode_err("a spline needs one weight per control point"));
                }
                d.write_b(self.rational_to_write(c));
                d.write_b(c.closed);
                d.write_b(c.periodic);
                d.write_bd(c.knot_tol);
                d.write_bd(c.ctrl_tol);
                d.write_bl(encode_count(c.knots.len(), "knots")?);
                d.write_bl(encode_count(c.points.len(), "control points")?);
                d.write_b(c.weights.is_some());
                for &k in &c.knots {
                    d.write_bd(k);
                }
                for (i, &p) in c.points.iter().enumerate() {
                    d.write_3bd(p);
                    if let Some(w) = &c.weights {
                        d.write_bd(w[i]);
                    }
                }
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Multilines.

/// A multiline (ODS 20.4.50).
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct MLine {
    /// DXF 40.
    pub scale: f64,
    /// Top 0, zero 1, bottom 2 (DXF 70).
    pub justification: u8,
    pub base_point: [f64; 3],
    pub extrusion: [f64; 3],
    /// DXF 71: has vertices 1, closed 2, start caps suppressed 4, end caps
    /// suppressed 8 (ODS: "open (1), closed (3)"). The "has vertices" bit is
    /// written from `vertices`, whatever this holds.
    pub flags: i16,
    /// The number of lines of the style (DXF 73); every vertex has as many.
    pub lines_in_style: u8,
    pub vertices: Vec<MLineVertex>,
    /// The MLINESTYLE (hard pointer).
    pub style: Handle,
}

/// One vertex of a multiline.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct MLineVertex {
    pub point: [f64; 3],
    /// The direction of the segment that starts here (DXF 12).
    pub direction: [f64; 3],
    /// The miter direction (DXF 13).
    pub miter: [f64; 3],
    /// One entry per line of the style.
    pub lines: Vec<MLineElement>,
}

/// The parameters of one line at one multiline vertex.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct MLineElement {
    /// DXF 41.
    pub segment_params: Vec<f64>,
    /// DXF 42.
    pub fill_params: Vec<f64>,
}

/// MLINE flag: the multiline has at least one vertex.
const MLINE_HAS_VERTICES: i16 = 1;

impl MLine {
    /// The flags to write: `flags` with the "has vertices" bit set exactly
    /// when there are vertices.
    fn stored_flags(&self) -> i16 {
        let has = if self.vertices.is_empty() {
            0
        } else {
            MLINE_HAS_VERTICES
        };
        self.flags & !MLINE_HAS_VERTICES | has
    }

    fn read_params(r: &mut ObjReader, what: &'static str) -> Result<Vec<f64>> {
        let n = count(i32::from(r.data.read_bs()?), what)?;
        check_fits(r, &[(n, MIN_BD)], what)?;
        (0..n).map(|_| r.data.read_bd()).collect()
    }

    pub(crate) fn decode(r: &mut ObjReader) -> Result<Self> {
        let d = &mut r.data;
        let scale = d.read_bd()?;
        let justification = d.read_rc()?;
        let base_point = d.read_3bd()?;
        let extrusion = d.read_3bd()?;
        let flags = d.read_bs()?;
        let lines_in_style = d.read_rc()?;
        let nv = count(i32::from(d.read_bs()?), "mline vertex")?;
        // Three 3BDs, and two BS counts per line.
        let vertex_bits = 3 * MIN_3BD + u64::from(lines_in_style) * 2 * MIN_BS;
        check_fits(r, &[(nv, vertex_bits)], "mline vertices")?;
        let mut vertices = Vec::with_capacity(nv);
        for _ in 0..nv {
            let d = &mut r.data;
            let point = d.read_3bd()?;
            let direction = d.read_3bd()?;
            let miter = d.read_3bd()?;
            let mut lines = Vec::with_capacity(usize::from(lines_in_style));
            for _ in 0..lines_in_style {
                lines.push(MLineElement {
                    segment_params: MLine::read_params(r, "mline segment parameters")?,
                    fill_params: MLine::read_params(r, "mline area fill parameters")?,
                });
            }
            vertices.push(MLineVertex {
                point,
                direction,
                miter,
                lines,
            });
        }
        Ok(MLine {
            scale,
            justification,
            base_point,
            extrusion,
            flags,
            lines_in_style,
            vertices,
            style: r.href_abs()?,
        })
    }

    pub(crate) fn encode(&self, w: &mut ObjWriter) -> Result<()> {
        let lines = usize::from(self.lines_in_style);
        if self.vertices.iter().any(|v| v.lines.len() != lines) {
            return Err(encode_err(format!(
                "every mline vertex needs {lines} lines, the style's count"
            )));
        }
        let d = &mut w.data;
        d.write_bd(self.scale);
        d.write_rc(self.justification);
        d.write_3bd(self.base_point);
        d.write_3bd(self.extrusion);
        d.write_bs(self.stored_flags());
        d.write_rc(self.lines_in_style);
        d.write_bs(short_count(self.vertices.len(), "mline vertices")?);
        for v in &self.vertices {
            d.write_3bd(v.point);
            d.write_3bd(v.direction);
            d.write_3bd(v.miter);
            for l in &v.lines {
                for params in [&l.segment_params, &l.fill_params] {
                    d.write_bs(short_count(params.len(), "mline parameters")?);
                    for &p in params {
                        d.write_bd(p);
                    }
                }
            }
        }
        put(w, HARD_POINTER, self.style)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::super::tests::roundtrip;
    use super::*;
    use crate::bits::BitWriter;
    use crate::handle::HandleRef;
    use crate::object::stream::tests::assemble;
    use crate::object::type_code;

    fn owned(from: u64, to: u64) -> EntityList {
        EntityList::Owned((from..=to).map(Handle).collect())
    }

    /// The reference codes of an encoding's handle stream.
    fn ref_codes(bytes: &[u8]) -> Vec<u8> {
        let (mut r, _) = ObjReader::split(bytes, Version::R2018, 30).unwrap();
        let mut codes = vec![];
        while r.handles.remaining_bits() >= 8 {
            codes.push(r.href().unwrap().code);
        }
        codes
    }

    fn encoded(f: impl Fn(&mut ObjWriter) -> Result<()>, code: u16) -> Vec<u8> {
        let mut w = ObjWriter::new(Handle(0x80));
        f(&mut w).unwrap();
        w.finish(code).unwrap()
    }

    pub(crate) fn polyline_2d() -> Polyline2d {
        Polyline2d {
            flags: 1,
            curve_type: 5,
            start_width: 0.5,
            end_width: 0.25,
            thickness: 2.0,
            elevation: 3.0,
            extrusion: [0.0, 1.0, 0.0],
            vertices: owned(0x101, 0x103),
            seqend: Handle(0x104),
        }
    }

    #[test]
    fn roundtrip_polyline_2d() {
        let mut p = polyline_2d();
        roundtrip(
            &p,
            Polyline2d::encode,
            Polyline2d::decode,
            type_code::POLYLINE_2D,
        );
        // No vertices; the BT and BE defaults.
        p.vertices = EntityList::Owned(vec![]);
        p.thickness = 0.0;
        p.extrusion = UNIT_Z;
        roundtrip(
            &p,
            Polyline2d::encode,
            Polyline2d::decode,
            type_code::POLYLINE_2D,
        );
    }

    #[test]
    fn roundtrip_polyline_3d() {
        let p = Polyline3d {
            spline_flags: 2,
            closed_flags: 1,
            vertices: owned(0x101, 0x105),
            seqend: Handle(0x106),
        };
        roundtrip(
            &p,
            Polyline3d::encode,
            Polyline3d::decode,
            type_code::POLYLINE_3D,
        );
    }

    #[test]
    fn roundtrip_polyface_mesh() {
        let p = PolyfaceMesh {
            vertex_count: 5,
            face_count: 2,
            vertices: owned(0x101, 0x107),
            seqend: Handle(0x108),
        };
        roundtrip(
            &p,
            PolyfaceMesh::encode,
            PolyfaceMesh::decode,
            type_code::POLYLINE_PFACE,
        );
    }

    #[test]
    fn roundtrip_polygon_mesh() {
        let p = PolygonMesh {
            flags: 16 | 1,
            curve_type: 6,
            m_count: 2,
            n_count: 3,
            m_density: 6,
            n_density: 7,
            vertices: owned(0x101, 0x106),
            seqend: Handle(0x107),
        };
        roundtrip(
            &p,
            PolygonMesh::encode,
            PolygonMesh::decode,
            type_code::POLYLINE_MESH,
        );
    }

    /// Every polyline's R2018 vertices and SEQEND are hard owners; ODS gives
    /// soft pointers for polyface and polygon mesh vertices, but the corpus
    /// has hard owners for polyface meshes (see `tests/polylines.rs`).
    #[test]
    fn polyline_vertices_are_hard_owners() {
        let pface = PolyfaceMesh {
            vertex_count: 1,
            face_count: 1,
            vertices: owned(0x101, 0x102),
            seqend: Handle(0x103),
        };
        let bytes = encoded(|w| pface.encode(w), type_code::POLYLINE_PFACE);
        assert_eq!(ref_codes(&bytes), [3, 3, 3]);
        let mesh = PolygonMesh {
            flags: 16,
            curve_type: 0,
            m_count: 1,
            n_count: 2,
            m_density: 0,
            n_density: 0,
            vertices: owned(0x101, 0x102),
            seqend: Handle(0x103),
        };
        let bytes = encoded(|w| mesh.encode(w), type_code::POLYLINE_MESH);
        assert_eq!(ref_codes(&bytes), [3, 3, 3]);
        let bytes = encoded(|w| polyline_2d().encode(w), type_code::POLYLINE_2D);
        assert_eq!(ref_codes(&bytes), [3, 3, 3, 3]);
    }

    #[test]
    fn linked_vertices_do_not_encode() {
        let mut p = polyline_2d();
        p.vertices = EntityList::Linked {
            first: Handle(0x101),
            last: Handle(0x103),
        };
        assert!(p.encode(&mut ObjWriter::new(Handle(1))).is_err());
    }

    /// R2000: no owned count; the first and last vertex (soft pointers),
    /// then the SEQEND. R2004: the count and every vertex.
    #[test]
    fn r2000_and_r2004_polyline_vertices() {
        let href = |h: &mut BitWriter, code: u8, value: u64| {
            h.write_handle_ref(HandleRef { code, value }).unwrap();
        };
        let mut d = BitWriter::new();
        d.write_rc(0);
        d.write_rc(1);
        let mut h = BitWriter::new();
        href(&mut h, 4, 0x101);
        href(&mut h, 4, 0x105);
        href(&mut h, 3, 0x106);
        let bytes = assemble(Version::R2000, type_code::POLYLINE_3D, &d, None, &h);
        let (mut r, _) = ObjReader::split(&bytes, Version::R2000, 30).unwrap();
        let p = Polyline3d::decode(&mut r).unwrap();
        assert_eq!(r.data.remaining_bits(), 0);
        assert_eq!(
            p.vertices,
            EntityList::Linked {
                first: Handle(0x101),
                last: Handle(0x105)
            }
        );
        assert_eq!((p.closed_flags, p.seqend), (1, Handle(0x106)));

        d.write_bl(2);
        let mut h = BitWriter::new();
        href(&mut h, 3, 0x101);
        href(&mut h, 3, 0x102);
        href(&mut h, 3, 0x103);
        let bytes = assemble(Version::R2004, type_code::POLYLINE_3D, &d, None, &h);
        let (mut r, _) = ObjReader::split(&bytes, Version::R2004, 30).unwrap();
        let p = Polyline3d::decode(&mut r).unwrap();
        assert_eq!((p.vertices, p.seqend), (owned(0x101, 0x102), Handle(0x103)));
    }

    #[test]
    fn vertex_count_is_bounded() {
        let mut d = BitWriter::new();
        d.write_bs(1);
        d.write_bs(1);
        d.write_bl(i32::MAX);
        let mut h = BitWriter::new();
        h.write_handle_ref(HandleRef { code: 3, value: 1 }).unwrap();
        let bytes = assemble(Version::R2018, type_code::POLYLINE_PFACE, &d, None, &h);
        let (mut r, _) = ObjReader::split(&bytes, Version::R2018, 30).unwrap();
        assert!(PolyfaceMesh::decode(&mut r).is_err());
    }

    fn vertex_2d() -> Vertex2d {
        Vertex2d {
            flags: 1,
            point: [1.0, 2.0, 0.0],
            start_width: 0.5,
            end_width: 0.75,
            bulge: 0.25,
            id: 7,
            tangent_dir: 1.5,
        }
    }

    #[test]
    fn roundtrip_vertex_2d() {
        let mut v = vertex_2d();
        roundtrip(&v, Vertex2d::encode, Vertex2d::decode, type_code::VERTEX_2D);
        // Equal widths, once negated; zero widths; -0.0 widths.
        for (s, e) in [(0.5, 0.5), (0.0, 0.0), (-0.0, -0.0), (0.0, 0.5)] {
            v.start_width = s;
            v.end_width = e;
            roundtrip(&v, Vertex2d::encode, Vertex2d::decode, type_code::VERTEX_2D);
        }
    }

    #[test]
    fn vertex_2d_equal_widths_are_stored_once() {
        let len = |s: f64, e: f64| {
            let mut w = ObjWriter::new(Handle(1));
            Vertex2d {
                start_width: s,
                end_width: e,
                ..vertex_2d()
            }
            .encode(&mut w)
            .unwrap();
            w.data.bit_len()
        };
        assert_eq!(len(0.5, 0.75), len(0.5, 0.5) + 66);
        // Zero widths are two short BDs.
        assert_eq!(len(0.0, 0.0), len(0.5, 0.5) - 64 + 2);
        let mut w = ObjWriter::new(Handle(1));
        let neg = Vertex2d {
            start_width: -1.0,
            ..vertex_2d()
        };
        assert!(neg.encode(&mut w).is_err());
    }

    /// Before R2010 a 2D vertex has no identifier. A negative start width
    /// gives both widths and no end width follows.
    #[test]
    fn r2004_vertex_2d() {
        let mut d = BitWriter::new();
        d.write_rc(0);
        d.write_3bd([1.0, 2.0, 0.0]);
        d.write_bd(-0.5);
        d.write_bd(0.25);
        d.write_bd(1.0);
        let bytes = assemble(
            Version::R2004,
            type_code::VERTEX_2D,
            &d,
            None,
            &BitWriter::new(),
        );
        let (mut r, _) = ObjReader::split(&bytes, Version::R2004, 30).unwrap();
        let v = Vertex2d::decode(&mut r).unwrap();
        assert_eq!(r.data.remaining_bits(), 0);
        assert_eq!(
            (v.start_width, v.end_width, v.bulge, v.id, v.tangent_dir),
            (0.5, 0.5, 0.25, 0, 1.0)
        );
    }

    #[test]
    fn roundtrip_vertex_3d_and_face() {
        for code in [
            type_code::VERTEX_3D,
            type_code::VERTEX_MESH,
            type_code::VERTEX_PFACE,
        ] {
            let v = Vertex3d {
                flags: 32,
                point: [1.0, -2.0, 3.5],
            };
            roundtrip(&v, Vertex3d::encode, Vertex3d::decode, code);
        }
        let f = VertexPfaceFace {
            indices: [1, -2, 3, 0],
        };
        roundtrip(
            &f,
            VertexPfaceFace::encode,
            VertexPfaceFace::decode,
            type_code::VERTEX_PFACE_FACE,
        );
    }

    pub(crate) fn lwpolyline() -> LwPolyline {
        LwPolyline {
            flags: LWPOLYLINE_CLOSED | LWPOLYLINE_PLINEGEN,
            const_width: 0.5,
            elevation: 2.0,
            thickness: 1.0,
            extrusion: [0.0, 1.0, 0.0],
            points: vec![[1.0, 2.0], [3.0, 2.0], [3.0, 5.0]],
            bulges: vec![0.0, 1.0, 0.0],
            vertex_ids: vec![1, 2, 3],
            widths: vec![[0.0, 0.0], [1.0, 2.0], [0.0, 0.0]],
            read_points: AsRead::default(),
        }
    }

    #[test]
    fn roundtrip_lwpolyline() {
        let mut p = lwpolyline();
        roundtrip(
            &p,
            LwPolyline::encode,
            LwPolyline::decode,
            type_code::LWPOLYLINE,
        );
        // Nothing optional stored; a stored -0.0 elevation; no points.
        p.const_width = 0.0;
        p.elevation = -0.0;
        p.thickness = 0.0;
        p.extrusion = UNIT_Z;
        p.bulges.clear();
        p.vertex_ids.clear();
        p.widths.clear();
        roundtrip(
            &p,
            LwPolyline::encode,
            LwPolyline::decode,
            type_code::LWPOLYLINE,
        );
        p.points.clear();
        roundtrip(
            &p,
            LwPolyline::encode,
            LwPolyline::decode,
            type_code::LWPOLYLINE,
        );
    }

    /// The stored flags word follows the fields: adding bulges or widths
    /// sets their bits, and bits in `flags` that say which fields are stored
    /// are ignored.
    #[test]
    fn lwpolyline_flags_follow_the_fields() {
        let word = |p: &LwPolyline| {
            let bytes = encoded(|w| p.encode(w), type_code::LWPOLYLINE);
            let (mut r, _) = ObjReader::split(&bytes, Version::R2018, 30).unwrap();
            r.data.read_bs().unwrap()
        };
        let mut p = LwPolyline {
            flags: LWPOLYLINE_CLOSED,
            const_width: 0.0,
            elevation: 0.0,
            thickness: 0.0,
            extrusion: UNIT_Z,
            points: vec![[0.0, 0.0], [1.0, 0.0]],
            bulges: vec![],
            vertex_ids: vec![],
            widths: vec![],
            read_points: AsRead::default(),
        };
        assert_eq!(word(&p), 0x200);
        p.bulges = vec![0.5, 0.0];
        assert_eq!(word(&p), 0x210);
        p.widths = vec![[0.0, 1.0], [0.0, 0.0]];
        assert_eq!(word(&p), 0x230);
        p.vertex_ids = vec![4, 5];
        p.elevation = 1.0;
        assert_eq!(word(&p), 0x638);
        p.flags |= 0x3F;
        p.elevation = -0.0;
        p.extrusion = [0.0, 0.0, -1.0];
        assert_eq!(word(&p), 0x639);
        p.const_width = 0.25;
        p.thickness = 3.0;
        assert_eq!(word(&p), 0x63F);
        let back = {
            let bytes = encoded(|w| p.encode(w), type_code::LWPOLYLINE);
            let (mut r, _) = ObjReader::split(&bytes, Version::R2018, 30).unwrap();
            LwPolyline::decode(&mut r).unwrap()
        };
        assert_eq!(back.flags, LWPOLYLINE_CLOSED);
    }

    /// Points after the first are DD pairs with the previous point as the
    /// default: a repeated point costs two 2-bit codes.
    #[test]
    fn lwpolyline_points_are_deltas() {
        let len = |points: Vec<[f64; 2]>| {
            let mut w = ObjWriter::new(Handle(1));
            LwPolyline {
                points,
                bulges: vec![],
                vertex_ids: vec![],
                widths: vec![],
                ..lwpolyline()
            }
            .encode(&mut w)
            .unwrap();
            w.data.bit_len()
        };
        assert_eq!(
            len(vec![[1.0, 2.0], [1.0, 2.0], [3.0, 5.0]]),
            len(vec![[1.0, 2.0], [3.0, 5.0], [3.0, 5.0]])
        );
        let one = len(vec![[1.0, 2.0]]);
        // The BL count stays 10 bits; the repeated point adds 4 bits.
        assert_eq!(len(vec![[1.0, 2.0], [1.0, 2.0]]), one + 4);
    }

    /// Before R2010 the vertex id bit has no list.
    #[test]
    fn r2004_lwpolyline_has_no_vertex_ids() {
        let mut d = BitWriter::new();
        d.write_bs(0x600);
        d.write_bl(1);
        d.write_2rd([1.0, 2.0]);
        let bytes = assemble(
            Version::R2004,
            type_code::LWPOLYLINE,
            &d,
            None,
            &BitWriter::new(),
        );
        let (mut r, _) = ObjReader::split(&bytes, Version::R2004, 30).unwrap();
        let p = LwPolyline::decode(&mut r).unwrap();
        assert_eq!(r.data.remaining_bits(), 0);
        assert_eq!((p.flags, p.points.len()), (LWPOLYLINE_CLOSED, 1));
        assert!(p.vertex_ids.is_empty());
    }

    #[test]
    fn lwpolyline_counts_are_bounded() {
        for counts in [[i32::MAX, 0], [2, i32::MAX], [-1, 0], [1, -1]] {
            let mut d = BitWriter::new();
            d.write_bs(LW_BULGES);
            d.write_bl(counts[0]);
            d.write_bl(counts[1]);
            d.write_2rd([1.0, 2.0]);
            d.write_2dd([1.0, 2.0], [1.0, 2.0]);
            d.write_bd(0.0);
            d.write_bd(0.0);
            let bytes = assemble(
                Version::R2018,
                type_code::LWPOLYLINE,
                &d,
                None,
                &BitWriter::new(),
            );
            let (mut r, _) = ObjReader::split(&bytes, Version::R2018, 30).unwrap();
            assert!(LwPolyline::decode(&mut r).is_err(), "{counts:?}");
        }
    }

    pub(crate) fn control_spline() -> Spline {
        Spline {
            degree: 3,
            flags1: SPLINE_FLAGS1_CLOSED,
            knot_parameter: SPLINE_KNOT_CUSTOM,
            form: SplineForm::Control(SplineControl {
                rational: true,
                closed: true,
                periodic: false,
                knot_tol: 1e-9,
                ctrl_tol: 1e-10,
                knots: vec![0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0],
                points: vec![[0.0; 3], [1.0, 2.0, 0.0], [3.0, 1.0, 0.0], [4.0, 4.0, 1.0]],
                weights: Some(vec![1.0, 0.5, 0.5, 1.0]),
            }),
            read: AsRead::default(),
        }
    }

    fn fit_spline() -> Spline {
        Spline {
            degree: 3,
            flags1: SPLINE_FLAGS1_METHOD_FIT | SPLINE_FLAGS1_USE_KNOT_PARAMETER,
            knot_parameter: 2,
            form: SplineForm::Fit(SplineFit {
                fit_tol: 1e-10,
                begin_tangent: [1.0, 0.0, 0.0],
                end_tangent: [0.0, 1.0, 0.0],
                points: vec![[0.0; 3], [5.0, 5.0, 0.0], [9.0, 2.0, 0.0]],
            }),
            read: AsRead::default(),
        }
    }

    fn reencode(s: &Spline) -> Spline {
        let bytes = encoded(|w| s.encode(w), type_code::SPLINE);
        let (mut r, _) = ObjReader::split(&bytes, Version::R2018, 30).unwrap();
        Spline::decode(&mut r).unwrap()
    }

    #[test]
    fn roundtrip_spline() {
        let mut s = control_spline();
        roundtrip(&s, Spline::encode, Spline::decode, type_code::SPLINE);
        if let SplineForm::Control(c) = &mut s.form {
            c.weights = None;
            c.rational = false;
        }
        roundtrip(&s, Spline::encode, Spline::decode, type_code::SPLINE);
        roundtrip(
            &fit_spline(),
            Spline::encode,
            Spline::decode,
            type_code::SPLINE,
        );
    }

    /// R2013+ files store fit data when the knot parameter is not custom
    /// and flags 1 has "use knot parameter", whatever the scenario says;
    /// the scenario is kept as read.
    #[test]
    fn r2013_spline_layout_follows_the_knot_parameter() {
        let mut d = BitWriter::new();
        d.write_bl(1); // scenario: 1, yet fit data follows
        d.write_bl(9);
        d.write_bl(2);
        d.write_bl(3);
        d.write_bd(1e-10);
        d.write_3bd([1.0, 0.0, 0.0]);
        d.write_3bd([0.0, 1.0, 0.0]);
        d.write_bl(2);
        d.write_3bd([0.0; 3]);
        d.write_3bd([1.0; 3]);
        let bytes = assemble(
            Version::R2013,
            type_code::SPLINE,
            &d,
            None,
            &BitWriter::new(),
        );
        let (mut r, _) = ObjReader::split(&bytes, Version::R2013, 30).unwrap();
        let s = Spline::decode(&mut r).unwrap();
        assert_eq!(r.data.remaining_bits(), 0);
        assert_eq!((s.flags1, s.knot_parameter, s.scenario()), (9, 2, 1));
        let SplineForm::Fit(f) = &s.form else {
            panic!("{s:?}");
        };
        assert_eq!(f.points, [[0.0; 3], [1.0; 3]]);
        let back = reencode(&s);
        assert_eq!(back, s);
        assert_eq!(back.stored_scenario(), Some(1));
    }

    /// Before R2013 the scenario decides; older splines get the R2013
    /// fields AutoCAD gives them.
    #[test]
    fn r2004_spline_scenarios() {
        let fit = {
            let mut d = BitWriter::new();
            d.write_bl(2);
            d.write_bl(3);
            d.write_bd(0.0);
            d.write_3bd([0.0; 3]);
            d.write_3bd([0.0; 3]);
            d.write_bl(1);
            d.write_3bd([1.0; 3]);
            d
        };
        let bytes = assemble(
            Version::R2004,
            type_code::SPLINE,
            &fit,
            None,
            &BitWriter::new(),
        );
        let (mut r, _) = ObjReader::split(&bytes, Version::R2004, 30).unwrap();
        let s = Spline::decode(&mut r).unwrap();
        assert_eq!(r.data.remaining_bits(), 0);
        assert!(s.form.is_fit());
        assert_eq!((s.flags1, s.knot_parameter, s.scenario()), (9, 0, 2));
        assert_eq!(reencode(&s), s);

        let mut control = BitWriter::new();
        control.write_bl(1);
        control.write_bl(3);
        // Not rational, closed, not periodic; the tolerances; no knots or
        // control points; no weights.
        for b in [false, true, false] {
            control.write_b(b);
        }
        control.write_bd(0.0);
        control.write_bd(0.0);
        control.write_bl(0);
        control.write_bl(0);
        control.write_b(false);
        let bytes = assemble(
            Version::R2004,
            type_code::SPLINE,
            &control,
            None,
            &BitWriter::new(),
        );
        let (mut r, _) = ObjReader::split(&bytes, Version::R2004, 30).unwrap();
        let s = Spline::decode(&mut r).unwrap();
        assert_eq!(r.data.remaining_bits(), 0);
        assert_eq!((s.flags1, s.knot_parameter, s.scenario()), (4, 15, 1));
        assert_eq!(reencode(&s), s);

        let mut bad = BitWriter::new();
        bad.write_bl(3);
        bad.write_bl(3);
        let bytes = assemble(
            Version::R2004,
            type_code::SPLINE,
            &bad,
            None,
            &BitWriter::new(),
        );
        let (mut r, _) = ObjReader::split(&bytes, Version::R2004, 30).unwrap();
        assert!(Spline::decode(&mut r).is_err());
    }

    /// The scenario, layout bits and closed bit are copies of the form: kept
    /// as read while it is unchanged, recomputed after an edit.
    #[test]
    fn spline_copies_follow_edits() {
        // A spline read with fit data, scenario 1, closed bit set.
        let mut s = fit_spline();
        s.flags1 |= SPLINE_FLAGS1_CLOSED;
        s.read = AsRead::new(SplineAsRead {
            fit: true,
            scenario: 1,
            closed: false,
            rational: false,
            weighted: false,
        });
        assert_eq!(s.scenario(), 1);
        assert_eq!(s.r2013_fields(), (13, 2));
        // Changed to control points: scenario 1, custom knots, layout bits
        // cleared, the closed bit following `closed`.
        let SplineForm::Control(c) = control_spline().form else {
            unreachable!()
        };
        s.form = SplineForm::Control(SplineControl {
            closed: false,
            ..c.clone()
        });
        assert_eq!(s.scenario(), 1);
        assert_eq!(s.r2013_fields(), (0, SPLINE_KNOT_CUSTOM));
        let back = reencode(&s);
        assert_eq!((back.flags1, back.knot_parameter), (0, SPLINE_KNOT_CUSTOM));
        assert_eq!(back.form, s.form);

        // Read closed with the closed bit: opening it clears the bit.
        let mut s = control_spline();
        s.read = AsRead::new(SplineAsRead {
            fit: false,
            scenario: 1,
            closed: true,
            rational: true,
            weighted: true,
        });
        assert_eq!(s.r2013_fields(), (4, SPLINE_KNOT_CUSTOM));
        if let SplineForm::Control(c) = &mut s.form {
            c.closed = false;
        }
        assert_eq!(s.r2013_fields(), (0, SPLINE_KNOT_CUSTOM));
        // Read closed without the closed bit (as another writer might store
        // it): kept while `closed` is unchanged.
        s.flags1 = 0;
        if let SplineForm::Control(c) = &mut s.form {
            c.closed = true;
        }
        assert_eq!(s.r2013_fields(), (0, SPLINE_KNOT_CUSTOM));
        // Built by hand: the bit follows `closed`.
        s.read = AsRead::default();
        assert_eq!(s.r2013_fields(), (4, SPLINE_KNOT_CUSTOM));

        // Built by hand: fit data with custom knots gets chord length and
        // the layout bits; scenario 2.
        let mut s = fit_spline();
        s.flags1 = 0;
        s.knot_parameter = SPLINE_KNOT_CUSTOM;
        assert_eq!(s.scenario(), 2);
        let back = reencode(&s);
        assert_eq!((back.flags1, back.knot_parameter), (9, SPLINE_KNOT_CHORD));
        assert_eq!(back.form, s.form);
        // Control points with fit-layout bits lose them.
        let mut s = control_spline();
        s.flags1 = 9;
        s.knot_parameter = 1;
        assert_eq!(s.r2013_fields(), (4, SPLINE_KNOT_CUSTOM));
    }

    #[test]
    fn spline_counts_are_bounded_and_weights_match() {
        let mut d = BitWriter::new();
        d.write_bl(1);
        d.write_bl(0);
        d.write_bl(15);
        d.write_bl(3);
        for _ in 0..3 {
            d.write_b(false);
        }
        d.write_bd(0.0);
        d.write_bd(0.0);
        d.write_bl(i32::MAX);
        d.write_bl(0);
        d.write_b(false);
        let bytes = assemble(
            Version::R2018,
            type_code::SPLINE,
            &d,
            None,
            &BitWriter::new(),
        );
        let (mut r, _) = ObjReader::split(&bytes, Version::R2018, 30).unwrap();
        assert!(Spline::decode(&mut r).is_err());

        let mut s = control_spline();
        if let SplineForm::Control(c) = &mut s.form {
            c.weights = Some(vec![1.0]);
        }
        assert!(s.encode(&mut ObjWriter::new(Handle(1))).is_err());
    }

    fn reencode_lw(p: &LwPolyline) -> LwPolyline {
        let bytes = encoded(|w| p.encode(w), type_code::LWPOLYLINE);
        let (mut r, _) = ObjReader::split(&bytes, Version::R2018, 30).unwrap();
        LwPolyline::decode(&mut r).unwrap()
    }

    /// Stored lists keep one entry per point: appending a point to a read
    /// polyline pads them, removing one cuts them; a list built by hand with
    /// another length is fitted too. Read lists of another length are kept
    /// while the point count is unchanged.
    #[test]
    fn lwpolyline_lists_follow_the_points() {
        let read = reencode_lw(&lwpolyline());
        assert_eq!(read.read_points.get(), Some(&3));
        // Append a point to the bulged polyline.
        let mut p = read.clone();
        p.points.push([7.0, 7.0]);
        let back = reencode_lw(&p);
        assert_eq!(back.points.len(), 4);
        assert_eq!(back.bulges, [0.0, 1.0, 0.0, 0.0]);
        assert_eq!(back.vertex_ids, [1, 2, 3, 0]);
        assert_eq!(back.widths, [[0.0; 2], [1.0, 2.0], [0.0; 2], [0.0; 2]]);
        // Remove a point.
        let mut p = read.clone();
        p.points.pop();
        let back = reencode_lw(&p);
        assert_eq!(back.bulges, [0.0, 1.0]);
        assert_eq!(back.vertex_ids, [1, 2]);
        assert_eq!(back.widths, [[0.0; 2], [1.0, 2.0]]);
        // Remove every point: nothing is stored.
        let mut p = read.clone();
        p.points.clear();
        let back = reencode_lw(&p);
        assert!(back.bulges.is_empty() && back.vertex_ids.is_empty() && back.widths.is_empty());
        // Built by hand with a short list.
        let mut p = lwpolyline();
        p.bulges = vec![0.5];
        assert_eq!(reencode_lw(&p).bulges, [0.5, 0.0, 0.0]);
        // Read with a short list (another writer): kept while the point
        // count is unchanged.
        let mut d = BitWriter::new();
        d.write_bs(LW_BULGES);
        d.write_bl(2);
        d.write_bl(1);
        d.write_2rd([0.0, 0.0]);
        d.write_2dd([1.0, 0.0], [0.0, 0.0]);
        d.write_bd(0.5);
        let bytes = assemble(
            Version::R2018,
            type_code::LWPOLYLINE,
            &d,
            None,
            &BitWriter::new(),
        );
        let (mut r, _) = ObjReader::split(&bytes, Version::R2018, 30).unwrap();
        let short = LwPolyline::decode(&mut r).unwrap();
        assert_eq!(short.bulges, [0.5]);
        let mut w = ObjWriter::new(Handle(0x80));
        short.encode(&mut w).unwrap();
        assert_eq!(w.data.into_bytes(), d.into_bytes());
    }

    /// Weights are stored exactly when `weights` is `Some`; `rational`
    /// follows, unless read disagreeing and unchanged.
    #[test]
    fn spline_rational_follows_the_weights() {
        let rational = |s: &Spline| match &reencode(s).form {
            SplineForm::Control(c) => c.rational,
            f => panic!("{f:?}"),
        };
        // Built by hand: weights without `rational`, `rational` without weights.
        let mut s = control_spline();
        edit_control(&mut s, |c| c.rational = false);
        assert!(rational(&s));
        edit_control(&mut s, |c| {
            c.rational = true;
            c.weights = None;
        });
        assert!(!rational(&s));
        // Read rational without weights: kept while unchanged; adding
        // weights or editing `rational` recomputes it.
        s.read = AsRead::new(SplineAsRead {
            fit: false,
            scenario: 1,
            closed: true,
            rational: true,
            weighted: false,
        });
        assert!(rational(&s));
        edit_control(&mut s, |c| c.rational = false);
        assert!(!rational(&s));
    }

    /// Change the control points of `s`.
    fn edit_control(s: &mut Spline, f: impl FnOnce(&mut SplineControl)) {
        match &mut s.form {
            SplineForm::Control(c) => f(c),
            SplineForm::Fit(_) => unreachable!(),
        }
    }

    pub(crate) fn mline() -> MLine {
        let element = |s: &[f64], f: &[f64]| MLineElement {
            segment_params: s.to_vec(),
            fill_params: f.to_vec(),
        };
        MLine {
            scale: 1.5,
            justification: 1,
            base_point: [1.0, 2.0, 0.0],
            extrusion: UNIT_Z,
            flags: 5,
            lines_in_style: 2,
            vertices: vec![
                MLineVertex {
                    point: [1.0, 2.0, 0.0],
                    direction: [1.0, 0.0, 0.0],
                    miter: [0.0, 1.0, 0.0],
                    lines: vec![
                        element(&[0.75, 0.0], &[]),
                        element(&[-0.75, 0.0, 2.0, 3.0], &[1.0]),
                    ],
                },
                MLineVertex {
                    point: [5.0, 2.0, 0.0],
                    direction: [1.0, 0.0, 0.0],
                    miter: [0.0, 1.0, 0.0],
                    lines: vec![element(&[], &[]), element(&[0.5], &[])],
                },
            ],
            style: Handle(0x18),
        }
    }

    #[test]
    fn roundtrip_mline() {
        let mut m = mline();
        roundtrip(&m, MLine::encode, MLine::decode, type_code::MLINE);
        m.vertices.clear();
        m.flags = 4;
        roundtrip(&m, MLine::encode, MLine::decode, type_code::MLINE);
    }

    /// The "has vertices" flag follows the vertices: removing them all
    /// clears it, adding one to an empty multiline sets it.
    #[test]
    fn mline_has_vertices_flag_follows_vertices() {
        let reencode = |m: &MLine| {
            let bytes = encoded(|w| m.encode(w), type_code::MLINE);
            let (mut r, _) = ObjReader::split(&bytes, Version::R2018, 30).unwrap();
            MLine::decode(&mut r).unwrap()
        };
        let mut m = mline();
        assert_eq!(reencode(&m).flags, 5);
        let vertices = std::mem::take(&mut m.vertices);
        let empty = reencode(&m);
        assert_eq!((empty.flags, empty.vertices.len()), (4, 0));
        let mut grown = empty;
        grown.vertices.push(vertices[0].clone());
        assert_eq!(reencode(&grown).flags, 5);
    }

    #[test]
    fn mline_lines_must_match_the_style() {
        let mut m = mline();
        m.lines_in_style = 3;
        assert!(m.encode(&mut ObjWriter::new(Handle(1))).is_err());
    }

    #[test]
    fn mline_counts_are_bounded() {
        let head = |d: &mut BitWriter, lines: u8, verts: i16| {
            d.write_bd(1.0);
            d.write_rc(0);
            d.write_3bd([0.0; 3]);
            d.write_3bd(UNIT_Z);
            d.write_bs(1);
            d.write_rc(lines);
            d.write_bs(verts);
        };
        let decode = |d: &BitWriter| {
            let bytes = assemble(Version::R2018, type_code::MLINE, d, None, &BitWriter::new());
            let (mut r, _) = ObjReader::split(&bytes, Version::R2018, 30).unwrap();
            MLine::decode(&mut r).is_err()
        };
        let mut d = BitWriter::new();
        head(&mut d, 255, i16::MAX);
        assert!(decode(&d));
        let mut d = BitWriter::new();
        head(&mut d, 1, -1);
        assert!(decode(&d));
        // One vertex whose segment parameter count exceeds the data.
        let mut d = BitWriter::new();
        head(&mut d, 1, 1);
        for _ in 0..3 {
            d.write_3bd([0.0; 3]);
        }
        d.write_bs(i16::MAX);
        d.write_bs(0);
        assert!(decode(&d));
    }
}
