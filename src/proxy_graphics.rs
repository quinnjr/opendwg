//! Proxy graphics: the drawing primitives that custom and proxy entities
//! (AEC, Civil 3D, tables, multileaders, meshes) store so a viewer that does
//! not know the entity can still draw it (ODS chapter 29).
//!
//! [`crate::object::EntityCommon::proxy_graphics`] keeps the raw bytes for
//! writing; [`decode`] turns them into [`ProxyPrimitive`]s on demand.
//!
//! # Layout
//!
//! Everything is little-endian. The blob is an `RL` total size (the whole blob,
//! these 4 bytes included), an `RL` primitive count, then per primitive an `RL`
//! size (the primitive's, header included), an `RL` opcode and the data. Data is
//! padded to 4 bytes. All of this was checked on the corpus, where the total
//! size equals the stored byte length and the count and sizes tile the blob
//! exactly, in every version from R2000 to R2018 (the R2010+ `BLL` byte count
//! in the entity common data agrees with the total size in every blob).
//!
//! # Where ODS is wrong or silent
//!
//! - A true color (opcode 22) is stored as one `RL`, not three `RC`: AutoCAD's
//!   packed color word, the method in the top byte (`0xC0` ByLayer, `0xC1`
//!   ByBlock, `0xC2` RGB, `0xC3` ACI index in the low bits, `0xC8` none), the
//!   same as a CMC color. [`ProxyPrimitive::SetTrueColor`] has the word and
//!   the [`crate::bits::Color`] decoded from it; `0xC0000000`, the corpus's
//!   most common value, is ByLayer.
//! - ODS gives no opcode number for "Unicode text 2": 38, the number after
//!   "Unknown 37", comes from the corpus.
//! - Opcodes 21 (empty) and 51 (one `RL`, 0 or 0x01000000 in the corpus) occur
//!   in the corpus and are not in ODS. They decode as [`ProxyPrimitive::Unknown`], as does
//!   37, which ODS lists as "Unknown, empty".
//! - A shell's face entries are a vertex count followed by that many vertex
//!   indices; the corpus's counts are positive. ODS says a count is negative;
//!   that sign marks a hole in the AcGi convention, so a face with a negative
//!   count is listed in [`ProxyPrimitive::Shell::hole_faces`].
//! - The edge, face and vertex trait lists after a mesh's vertices or a shell's
//!   faces are validated only by the primitive's size and are not decoded.
//!   PUSH_MODELXFORM2's trailing unknown data is not decoded either. The raw
//!   blob keeps both.
//!
//! # Text encoding
//!
//! Padded strings (PS: opcodes 10 and 11) are zero-terminated, padded to 4
//! bytes, and in the drawing's code page (ODS); the padding bytes are not
//! zero in real files and are ignored. Padded Unicode strings (PUS: opcodes 36
//! and 38) are UTF-16LE, terminated by two zero bytes and padded to 4 bytes.
//! The corpus has code page text (11) in R2000 and R2004 files and Unicode
//! text (38) from R2007, and AEC entities use 36, so the opcode, not the
//! version, tells the encoding. The length `RL` of the second text form is
//! not used: the terminator is.
//!
//! # Hostile input
//!
//! Every count is checked against the bytes left in its primitive before
//! anything is allocated, and every primitive size against the bytes left in
//! the blob. Records do not nest (an embedded LWPOLYLINE is kept as bytes), so
//! there is no recursion. An opcode this module does not know becomes
//! [`ProxyPrimitive::Unknown`] and decoding goes on. A record whose size
//! does not fit the blob is an error (framing is fatal). A record whose data
//! cannot be decoded for its opcode (too short, a string without terminator, a
//! layout the corpus does not confirm) becomes [`ProxyPrimitive::Undecoded`]
//! with its data and the reason, so the other primitives still draw.
//!
//! Strings that were not valid in their encoding (malformed code page bytes,
//! unpaired UTF-16 surrogates) are decoded with U+FFFD and flagged `lossy`.
//! A blob that ends early is an error.

use crate::bits::{Color, cmc_from_parts};
use crate::error::{Error, Result};
use crate::object::EntityCommon;
use crate::object::LwPolyline;
use crate::version::Version;

/// The opcodes of ODS chapter 29.
#[allow(missing_docs)] // the ODS names
pub mod opcode {
    pub const EXTENTS: u32 = 1;
    pub const CIRCLE: u32 = 2;
    pub const CIRCLE_3PT: u32 = 3;
    pub const CIRCULAR_ARC: u32 = 4;
    pub const CIRCULAR_ARC_3PT: u32 = 5;
    pub const POLYLINE: u32 = 6;
    pub const POLYGON: u32 = 7;
    pub const MESH: u32 = 8;
    pub const SHELL: u32 = 9;
    pub const TEXT: u32 = 10;
    pub const TEXT2: u32 = 11;
    pub const XLINE: u32 = 12;
    pub const RAY: u32 = 13;
    pub const SUBENT_COLOR: u32 = 14;
    pub const SUBENT_LAYER: u32 = 16;
    pub const SUBENT_LINETYPE: u32 = 18;
    pub const SUBENT_MARKER: u32 = 19;
    pub const SUBENT_FILL_ON: u32 = 20;
    pub const SUBENT_TRUE_COLOR: u32 = 22;
    pub const SUBENT_LINEWEIGHT: u32 = 23;
    pub const SUBENT_LTSCALE: u32 = 24;
    pub const SUBENT_THICKNESS: u32 = 25;
    pub const SUBENT_PLOT_STYLE: u32 = 26;
    pub const PUSH_CLIP: u32 = 27;
    pub const POP_CLIP: u32 = 28;
    pub const PUSH_MODEL_XFORM: u32 = 29;
    pub const PUSH_MODEL_XFORM2: u32 = 30;
    pub const POP_MODEL_XFORM: u32 = 31;
    pub const POLYLINE_WITH_NORMAL: u32 = 32;
    pub const LWPOLYLINE: u32 = 33;
    pub const SUBENT_MATERIAL: u32 = 34;
    pub const SUBENT_MAPPER: u32 = 35;
    pub const UNICODE_TEXT: u32 = 36;
    pub const UNICODE_TEXT2: u32 = 38;
}

/// One proxy graphics primitive. Coordinates are in the entity's space (WCS
/// unless a transform or clip is pushed). The `Set...` items change the state
/// of the primitives that follow.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub enum ProxyPrimitive {
    /// Extents: minimum and maximum corners.
    Extents([f64; 3], [f64; 3]),
    Circle {
        center: [f64; 3],
        radius: f64,
        normal: [f64; 3],
    },
    Circle3Pt([[f64; 3]; 3]),
    CircularArc {
        center: [f64; 3],
        radius: f64,
        normal: [f64; 3],
        start_vector: [f64; 3],
        sweep: f64,
        arc_type: i32,
    },
    CircularArc3Pt {
        points: [[f64; 3]; 3],
        arc_type: i32,
    },
    Polyline(Vec<[f64; 3]>),
    Polygon(Vec<[f64; 3]>),
    /// A polyline with a normal vector (opcode 32).
    PolylineWithNormal {
        points: Vec<[f64; 3]>,
        normal: [f64; 3],
    },
    /// A grid of `rows` x `cols` vertices, row by row.
    Mesh {
        rows: u32,
        cols: u32,
        vertices: Vec<[f64; 3]>,
    },
    Shell {
        vertices: Vec<[f64; 3]>,
        /// The vertex indices of each face.
        faces: Vec<Vec<i32>>,
        /// The faces (indices into `faces`) stored with a negative count.
        hole_faces: Vec<usize>,
    },
    /// Text of opcode 10 (code page) or 36 (Unicode).
    Text {
        position: [f64; 3],
        normal: [f64; 3],
        direction: [f64; 3],
        height: f64,
        width_factor: f64,
        oblique: f64,
        text: String,
        /// The string had malformed bytes, replaced with U+FFFD.
        lossy: bool,
    },
    /// Text of opcode 11 (code page) or 38 (Unicode).
    Text2(Box<ProxyText2>),
    Xline([f64; 3], [f64; 3]),
    Ray([f64; 3], [f64; 3]),
    SetColor(i32),
    SetLayer(i32),
    /// The linetype index; -1 (0xFFFFFFFF) is BYLAYER, -2 BYBLOCK.
    SetLinetype(i32),
    SetMarker(i32),
    SetFillMode(bool),
    /// AutoCAD's packed color word and the color it decodes to (see the
    /// module documentation).
    SetTrueColor {
        word: u32,
        /// The color the word stands for; `None` for an unknown method byte.
        color: Option<Color>,
    },
    SetLineweight(i32),
    SetLtScale(f64),
    SetThickness(f64),
    /// Plot style: 0 BYLAYER, 1 BYBLOCK, 2 dictionary default, 3 by id.
    SetPlotStyle {
        kind: i32,
        index: i32,
    },
    /// The material handle, as the 8 stored bytes read little-endian.
    SetMaterial(u64),
    SetMapper {
        projection: i32,
        u_tiling: i32,
        v_tiling: i32,
        auto_transform: i32,
    },
    PushClip(Box<ProxyClip>),
    PopClip,
    PushModelTransform([f64; 16]),
    PopModelTransform,
    /// The embedded LWPOLYLINE data, from its flags `BS` to its width array;
    /// [`ProxyPrimitive::lw_polyline`] decodes it.
    LwPolyline(Vec<u8>),
    /// An opcode not decoded here, with its data.
    Unknown {
        opcode: u32,
        data: Vec<u8>,
    },
    /// A known opcode whose data could not be decoded (too short, an
    /// unterminated string, a layout nothing confirms), with its data and the
    /// reason. The other primitives of the blob are unaffected.
    Undecoded {
        opcode: u32,
        data: Vec<u8>,
        error: String,
    },
}

/// The second text form (opcodes 11 and 38).
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct ProxyText2 {
    pub position: [f64; 3],
    pub normal: [f64; 3],
    pub direction: [f64; 3],
    pub text: String,
    /// A string of the primitive had malformed bytes, replaced with U+FFFD.
    pub lossy: bool,
    /// Do not interpret `%%` codes.
    pub raw: bool,
    pub height: f64,
    pub width_factor: f64,
    pub oblique: f64,
    pub tracking: f64,
    pub backwards: bool,
    pub upside_down: bool,
    pub vertical: bool,
    pub underlined: bool,
    pub overlined: bool,
    /// The TrueType font descriptor, which only the Unicode form (38) has.
    pub font: Option<ProxyFont>,
    pub font_file: String,
    pub big_font_file: String,
}

/// The TrueType font descriptor of a Unicode text.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct ProxyFont {
    pub bold: bool,
    pub italic: bool,
    pub charset: u8,
    pub pitch_and_family: u8,
    pub type_face: String,
}

/// A clip boundary pushed by PUSH_CLIP (opcode 27).
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct ProxyClip {
    pub extrusion: [f64; 3],
    pub origin: [f64; 3],
    pub points: Vec<[f64; 2]>,
    pub transform: [f64; 16],
    pub inverse_block_transform: [f64; 16],
    pub front_clip_on: bool,
    pub back_clip_on: bool,
    pub front: f64,
    pub back: f64,
    pub draw_boundary: bool,
}

/// Bytes of a primitive: a bounds-checked cursor.
struct Cur<'a> {
    b: &'a [u8],
    at: usize,
    /// A string read so far had malformed bytes.
    lossy: bool,
}

const TRUNC: Error = Error::Truncated {
    context: "proxy graphics",
};

impl<'a> Cur<'a> {
    fn new(b: &'a [u8]) -> Self {
        Cur {
            b,
            at: 0,
            lossy: false,
        }
    }

    fn left(&self) -> usize {
        self.b.len() - self.at
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let s = self
            .b
            .get(self.at..)
            .and_then(|s| s.get(..n))
            .ok_or(TRUNC)?;
        self.at += n;
        Ok(s)
    }

    fn rest(&mut self) -> &'a [u8] {
        let s = &self.b[self.at..];
        self.at = self.b.len();
        s
    }

    fn u32(&mut self) -> Result<u32> {
        let s = self.take(4)?;
        Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
    }

    fn i32(&mut self) -> Result<i32> {
        Ok(self.u32()? as i32)
    }

    fn flag(&mut self) -> Result<bool> {
        Ok(self.u32()? != 0)
    }

    fn f64(&mut self) -> Result<f64> {
        let s = self.take(8)?;
        let mut a = [0u8; 8];
        a.copy_from_slice(s);
        Ok(f64::from_le_bytes(a))
    }

    fn p3(&mut self) -> Result<[f64; 3]> {
        Ok([self.f64()?, self.f64()?, self.f64()?])
    }

    fn matrix(&mut self) -> Result<[f64; 16]> {
        let mut m = [0.0; 16];
        for v in &mut m {
            *v = self.f64()?;
        }
        Ok(m)
    }

    /// An `RL` count of items of `size` bytes each, checked against the bytes
    /// left before anything is allocated.
    fn count(&mut self, size: usize) -> Result<usize> {
        let n = usize::try_from(self.u32()?).map_err(|_| TRUNC)?;
        match n.checked_mul(size) {
            Some(bytes) if bytes <= self.left() => Ok(n),
            _ => Err(TRUNC),
        }
    }

    fn points(&mut self, n: usize) -> Result<Vec<[f64; 3]>> {
        (0..n).map(|_| self.p3()).collect()
    }

    /// A padded string (PS): bytes up to a zero, then padding to 4 bytes.
    fn ps(&mut self, codepage: u16) -> Result<String> {
        let rest = &self.b[self.at..];
        let len = rest.iter().position(|&b| b == 0).ok_or(TRUNC)?;
        let (text, malformed) = crate::strings::decode_checked(codepage, &rest[..len]);
        self.lossy |= malformed;
        self.at += (len + 1).next_multiple_of(4).min(rest.len());
        Ok(text)
    }

    /// A padded Unicode string (PUS): UTF-16LE up to a zero unit, then
    /// padding to 4 bytes.
    fn pus(&mut self) -> Result<String> {
        let rest = &self.b[self.at..];
        let len = rest
            .chunks_exact(2)
            .position(|u| u == [0, 0])
            .ok_or(TRUNC)?
            * 2;
        let (text, replaced) = crate::strings::decode_utf16le_checked(&rest[..len]);
        self.lossy |= replaced;
        self.at += (len + 2).next_multiple_of(4).min(rest.len());
        Ok(text)
    }
}

/// Decode a proxy graphics blob, reading code page strings as Windows-1252
/// (see [`decode_with_codepage`]).
pub fn decode(bytes: &[u8], version: Version) -> Result<Vec<ProxyPrimitive>> {
    decode_with_codepage(bytes, version, 30)
}

/// Decode a proxy graphics blob of a drawing whose strings use `codepage`
/// (the DWG code page index, 30 for Windows-1252). The layout is the same in
/// every supported version, so `version` is not used.
pub fn decode_with_codepage(
    bytes: &[u8],
    _version: Version,
    codepage: u16,
) -> Result<Vec<ProxyPrimitive>> {
    let mut head = Cur::new(bytes);
    let total = usize::try_from(head.u32()?).map_err(|_| TRUNC)?;
    let count = usize::try_from(head.u32()?).map_err(|_| TRUNC)?;
    if total < 8 {
        return Err(Error::Corrupt(
            "proxy graphics size below its header".into(),
        ));
    }
    let blob = bytes.get(..total).ok_or(TRUNC)?;
    // Every primitive takes at least its 8-byte header.
    if count > (total - 8) / 8 {
        return Err(TRUNC);
    }
    let mut c = Cur::new(&blob[8..]);
    let mut out = Vec::with_capacity(count.min(64));
    for _ in 0..count {
        let size = usize::try_from(c.u32()?).map_err(|_| TRUNC)?;
        let op = c.u32()?;
        let data_len = size.checked_sub(8).ok_or_else(|| {
            Error::Corrupt("proxy graphics primitive smaller than its header".into())
        })?;
        let data = c.take(data_len)?;
        out.push(
            primitive(op, data, codepage).unwrap_or_else(|e| ProxyPrimitive::Undecoded {
                opcode: op,
                data: data.to_vec(),
                error: e.to_string(),
            }),
        );
    }
    Ok(out)
}

fn primitive(op: u32, data: &[u8], codepage: u16) -> Result<ProxyPrimitive> {
    use ProxyPrimitive as P;
    let mut c = Cur::new(data);
    let c = &mut c;
    Ok(match op {
        opcode::EXTENTS => P::Extents(c.p3()?, c.p3()?),
        opcode::CIRCLE => P::Circle {
            center: c.p3()?,
            radius: c.f64()?,
            normal: c.p3()?,
        },
        opcode::CIRCLE_3PT => P::Circle3Pt([c.p3()?, c.p3()?, c.p3()?]),
        opcode::CIRCULAR_ARC => P::CircularArc {
            center: c.p3()?,
            radius: c.f64()?,
            normal: c.p3()?,
            start_vector: c.p3()?,
            sweep: c.f64()?,
            arc_type: c.i32()?,
        },
        opcode::CIRCULAR_ARC_3PT => P::CircularArc3Pt {
            points: [c.p3()?, c.p3()?, c.p3()?],
            arc_type: c.i32()?,
        },
        opcode::POLYLINE => {
            let n = c.count(24)?;
            P::Polyline(c.points(n)?)
        }
        opcode::POLYGON => {
            let n = c.count(24)?;
            P::Polygon(c.points(n)?)
        }
        opcode::POLYLINE_WITH_NORMAL => {
            let n = c.count(24)?;
            P::PolylineWithNormal {
                points: c.points(n)?,
                normal: c.p3()?,
            }
        }
        opcode::MESH => {
            let rows = c.u32()?;
            let cols = c.u32()?;
            let n = usize::try_from(rows)
                .ok()
                .zip(usize::try_from(cols).ok())
                .and_then(|(r, k)| r.checked_mul(k))
                .filter(|n| n.checked_mul(24).is_some_and(|b| b <= c.left()))
                .ok_or(TRUNC)?;
            P::Mesh {
                rows,
                cols,
                vertices: c.points(n)?,
            }
        }
        opcode::SHELL => {
            let n = c.count(24)?;
            let vertices = c.points(n)?;
            // Each entry is 4 bytes, so the count is bounded by the bytes left.
            let entries = c.count(4)?;
            let mut faces = Vec::new();
            let mut hole_faces = Vec::new();
            let mut used = 0;
            while used < entries {
                let len = c.i32()?;
                used += 1;
                let m = usize::try_from(len.unsigned_abs()).map_err(|_| TRUNC)?;
                if m > entries - used {
                    return Err(TRUNC);
                }
                if len < 0 {
                    hole_faces.push(faces.len());
                }
                faces.push((0..m).map(|_| c.i32()).collect::<Result<Vec<_>>>()?);
                used += m;
            }
            P::Shell {
                vertices,
                faces,
                hole_faces,
            }
        }
        opcode::TEXT | opcode::UNICODE_TEXT => {
            let position = c.p3()?;
            let normal = c.p3()?;
            let direction = c.p3()?;
            let height = c.f64()?;
            let width_factor = c.f64()?;
            let oblique = c.f64()?;
            let text = if op == opcode::TEXT {
                c.ps(codepage)?
            } else {
                c.pus()?
            };
            P::Text {
                position,
                normal,
                direction,
                height,
                width_factor,
                oblique,
                text,
                lossy: c.lossy,
            }
        }
        opcode::TEXT2 | opcode::UNICODE_TEXT2 => {
            let unicode = op == opcode::UNICODE_TEXT2;
            let position = c.p3()?;
            let normal = c.p3()?;
            let direction = c.p3()?;
            let text = if unicode { c.pus()? } else { c.ps(codepage)? };
            let _length = c.i32()?;
            let raw = c.u32()? == 0;
            let height = c.f64()?;
            let width_factor = c.f64()?;
            let oblique = c.f64()?;
            let tracking = c.f64()?;
            let backwards = c.flag()?;
            let upside_down = c.flag()?;
            let vertical = c.flag()?;
            let underlined = c.flag()?;
            let overlined = c.flag()?;
            let (font, font_file, big_font_file);
            if unicode {
                let bold = c.flag()?;
                let italic = c.flag()?;
                let charset = c.u32()? as u8;
                let pitch_and_family = c.u32()? as u8;
                let type_face = c.pus()?;
                font = Some(ProxyFont {
                    bold,
                    italic,
                    charset,
                    pitch_and_family,
                    type_face,
                });
                font_file = c.pus()?;
                big_font_file = c.pus()?;
            } else {
                font = None;
                font_file = c.ps(codepage)?;
                big_font_file = c.ps(codepage)?;
            }
            P::Text2(Box::new(ProxyText2 {
                position,
                normal,
                direction,
                text,
                lossy: c.lossy,
                raw,
                height,
                width_factor,
                oblique,
                tracking,
                backwards,
                upside_down,
                vertical,
                underlined,
                overlined,
                font,
                font_file,
                big_font_file,
            }))
        }
        opcode::XLINE => P::Xline(c.p3()?, c.p3()?),
        opcode::RAY => P::Ray(c.p3()?, c.p3()?),
        opcode::SUBENT_COLOR => P::SetColor(c.i32()?),
        opcode::SUBENT_LAYER => P::SetLayer(c.i32()?),
        opcode::SUBENT_LINETYPE => P::SetLinetype(c.i32()?),
        opcode::SUBENT_MARKER => P::SetMarker(c.i32()?),
        opcode::SUBENT_FILL_ON => P::SetFillMode(c.flag()?),
        opcode::SUBENT_TRUE_COLOR => {
            let word = c.u32()?;
            P::SetTrueColor {
                word,
                color: cmc_from_parts(word, 0, String::new(), String::new()).ok(),
            }
        }
        opcode::SUBENT_LINEWEIGHT => P::SetLineweight(c.i32()?),
        opcode::SUBENT_LTSCALE => P::SetLtScale(c.f64()?),
        opcode::SUBENT_THICKNESS => P::SetThickness(c.f64()?),
        opcode::SUBENT_PLOT_STYLE => P::SetPlotStyle {
            kind: c.i32()?,
            index: c.i32()?,
        },
        opcode::PUSH_CLIP => {
            let extrusion = c.p3()?;
            let origin = c.p3()?;
            let n = c.count(16)?;
            let points = (0..n)
                .map(|_| Ok([c.f64()?, c.f64()?]))
                .collect::<Result<_>>()?;
            P::PushClip(Box::new(ProxyClip {
                extrusion,
                origin,
                points,
                transform: c.matrix()?,
                inverse_block_transform: c.matrix()?,
                front_clip_on: c.flag()?,
                back_clip_on: c.flag()?,
                front: c.f64()?,
                back: c.f64()?,
                draw_boundary: c.flag()?,
            }))
        }
        opcode::POP_CLIP => P::PopClip,
        // PUSH_MODELXFORM2's unknown trailing data is left out.
        opcode::PUSH_MODEL_XFORM | opcode::PUSH_MODEL_XFORM2 => P::PushModelTransform(c.matrix()?),
        opcode::POP_MODEL_XFORM => P::PopModelTransform,
        opcode::LWPOLYLINE => {
            let n = c.count(1)?;
            P::LwPolyline(c.take(n)?.to_vec())
        }
        opcode::SUBENT_MATERIAL => {
            let s = c.take(8)?;
            let mut a = [0u8; 8];
            a.copy_from_slice(s);
            P::SetMaterial(u64::from_le_bytes(a))
        }
        opcode::SUBENT_MAPPER => {
            let _dummy1 = c.i32()?;
            let _dummy2 = c.i32()?;
            let projection = c.i32()?;
            let u_tiling = c.i32()?;
            let v_tiling = c.i32()?;
            let auto_transform = c.i32()?;
            P::SetMapper {
                projection,
                u_tiling,
                v_tiling,
                auto_transform,
            }
        }
        _ => P::Unknown {
            opcode: op,
            data: c.rest().to_vec(),
        },
    })
}

impl ProxyPrimitive {
    /// Decode the embedded data of a [`ProxyPrimitive::LwPolyline`] with the
    /// LWPOLYLINE decoder, as it reads an entity of `version` (vertex ids are
    /// read from R2010). `None` for any other primitive.
    pub fn lw_polyline(&self, version: Version) -> Option<Result<LwPolyline>> {
        let ProxyPrimitive::LwPolyline(bytes) = self else {
            return None;
        };
        Some(LwPolyline::decode_embedded(bytes, version))
    }
}

impl EntityCommon {
    /// The decoded proxy graphics of the entity, `None` when it has none.
    /// Strings are read as Windows-1252; see [`EntityCommon::proxy_primitives_with_codepage`].
    pub fn proxy_primitives(&self, v: Version) -> Option<Result<Vec<ProxyPrimitive>>> {
        self.proxy_graphics.as_deref().map(|b| decode(b, v))
    }

    /// Like [`EntityCommon::proxy_primitives`], reading code page strings with the
    /// drawing's `codepage`.
    pub fn proxy_primitives_with_codepage(
        &self,
        v: Version,
        codepage: u16,
    ) -> Option<Result<Vec<ProxyPrimitive>>> {
        self.proxy_graphics
            .as_deref()
            .map(|b| decode_with_codepage(b, v, codepage))
    }
}

#[cfg(test)]
mod tests {
    use super::ProxyPrimitive as P;
    use super::*;
    use crate::bits::Color;
    use crate::object::ObjWriter;

    const V: Version = Version::R2018;

    fn f(v: f64) -> Vec<u8> {
        v.to_le_bytes().to_vec()
    }

    fn l(v: i32) -> Vec<u8> {
        v.to_le_bytes().to_vec()
    }

    fn p3(p: [f64; 3]) -> Vec<u8> {
        p.iter().flat_map(|&v| f(v)).collect()
    }

    fn cat(parts: &[Vec<u8>]) -> Vec<u8> {
        parts.concat()
    }

    /// One primitive: size, opcode, data.
    fn prim(op: u32, data: &[u8]) -> Vec<u8> {
        let mut v = l(data.len() as i32 + 8);
        v.extend(l(op as i32));
        v.extend_from_slice(data);
        v
    }

    /// A blob of primitives: total size, count, the primitives.
    fn blob(prims: &[Vec<u8>]) -> Vec<u8> {
        let body = prims.concat();
        let mut v = l(body.len() as i32 + 8);
        v.extend(l(prims.len() as i32));
        v.extend(body);
        v
    }

    fn one(op: u32, data: &[u8]) -> P {
        let mut out = decode(&blob(&[prim(op, data)]), V).unwrap();
        assert_eq!(out.len(), 1);
        out.remove(0)
    }

    /// PS or PUS text padded to 4 bytes, with garbage padding as in real files.
    fn ps(bytes: &[u8], terminator: usize) -> Vec<u8> {
        let mut v = bytes.to_vec();
        v.extend(vec![0; terminator]);
        while v.len() % 4 != 0 {
            v.push(0x7F);
        }
        v
    }

    fn pus(s: &str) -> Vec<u8> {
        let b: Vec<u8> = s.encode_utf16().flat_map(u16::to_le_bytes).collect();
        ps(&b, 2)
    }

    const A: [f64; 3] = [1.0, 2.0, 3.0];
    const B: [f64; 3] = [4.0, 5.0, 6.0];
    const C: [f64; 3] = [7.0, 8.0, 9.0];

    #[test]
    fn empty_blob_has_no_primitives() {
        assert_eq!(decode(&blob(&[]), V).unwrap(), vec![]);
    }

    #[test]
    fn geometry_opcodes() {
        assert_eq!(
            one(opcode::EXTENTS, &cat(&[p3(A), p3(B)])),
            P::Extents(A, B)
        );
        assert_eq!(
            one(opcode::CIRCLE, &cat(&[p3(A), f(2.5), p3(B)])),
            P::Circle {
                center: A,
                radius: 2.5,
                normal: B
            }
        );
        assert_eq!(
            one(opcode::CIRCLE_3PT, &cat(&[p3(A), p3(B), p3(C)])),
            P::Circle3Pt([A, B, C])
        );
        assert_eq!(
            one(
                opcode::CIRCULAR_ARC,
                &cat(&[p3(A), f(2.5), p3(B), p3(C), f(1.5), l(2)])
            ),
            P::CircularArc {
                center: A,
                radius: 2.5,
                normal: B,
                start_vector: C,
                sweep: 1.5,
                arc_type: 2
            }
        );
        assert_eq!(
            one(opcode::CIRCULAR_ARC_3PT, &cat(&[p3(A), p3(B), p3(C), l(1)])),
            P::CircularArc3Pt {
                points: [A, B, C],
                arc_type: 1
            }
        );
        let pts = cat(&[l(2), p3(A), p3(B)]);
        assert_eq!(one(opcode::POLYLINE, &pts), P::Polyline(vec![A, B]));
        assert_eq!(one(opcode::POLYGON, &pts), P::Polygon(vec![A, B]));
        assert_eq!(
            one(opcode::POLYLINE_WITH_NORMAL, &cat(&[pts, p3(C)])),
            P::PolylineWithNormal {
                points: vec![A, B],
                normal: C
            }
        );
        assert_eq!(one(opcode::XLINE, &cat(&[p3(A), p3(B)])), P::Xline(A, B));
        assert_eq!(one(opcode::RAY, &cat(&[p3(A), p3(B)])), P::Ray(A, B));
    }

    #[test]
    fn mesh_and_shell() {
        // Trait lists after the vertices are not decoded.
        let mesh = cat(&[
            l(2),
            l(3),
            (0..6).flat_map(|i| p3([f64::from(i); 3])).collect(),
            l(0),
        ]);
        let P::Mesh {
            rows,
            cols,
            vertices,
        } = one(opcode::MESH, &mesh)
        else {
            panic!()
        };
        assert_eq!((rows, cols, vertices.len()), (2, 3, 6));
        assert_eq!(vertices[5], [5.0; 3]);
        // A triangle, then a hole with a negative count, then a face the
        // trait lists follow.
        let shell = cat(&[
            l(4),
            p3(A),
            p3(B),
            p3(C),
            p3([0.0; 3]),
            l(8),
            l(3),
            l(0),
            l(1),
            l(2),
            l(-3),
            l(1),
            l(2),
            l(3),
            l(0),
            l(0),
            l(0),
        ]);
        assert_eq!(
            one(opcode::SHELL, &shell),
            P::Shell {
                vertices: vec![A, B, C, [0.0; 3]],
                faces: vec![vec![0, 1, 2], vec![1, 2, 3]],
                hole_faces: vec![1]
            }
        );
    }

    #[test]
    fn text_opcodes() {
        let head = cat(&[p3(A), p3(B), p3(C)]);
        // Code page text (opcode 10) in Shift-JIS (code page 22).
        let sjis = crate::strings::encode(22, "日本語");
        let ten = cat(&[head.clone(), f(2.0), f(0.8), f(0.1), ps(&sjis, 1)]);
        let want = |text: &str| P::Text {
            position: A,
            normal: B,
            direction: C,
            height: 2.0,
            width_factor: 0.8,
            oblique: 0.1,
            text: text.into(),
            lossy: false,
        };
        let out = decode_with_codepage(&blob(&[prim(opcode::TEXT, &ten)]), V, 22).unwrap();
        assert_eq!(out, vec![want("日本語")]);
        // Unicode text (36) with a surrogate pair.
        let t36 = cat(&[head.clone(), f(2.0), f(0.8), f(0.1), pus("A\u{1F600}b")]);
        assert_eq!(one(opcode::UNICODE_TEXT, &t36), want("A\u{1F600}b"));
        // A string that fills its 4 bytes exactly is followed by its terminator
        // in the next 4.
        let t = cat(&[head.clone(), f(2.0), f(0.8), f(0.1), ps(b"abcd", 1)]);
        assert_eq!(one(opcode::TEXT, &t), want("abcd"));

        let tail = cat(&[
            l(3),
            l(1),
            f(2.0),
            f(0.8),
            f(0.1),
            f(1.5),
            l(1),
            l(0),
            l(1),
            l(0),
            l(1),
        ]);
        let t11 = cat(&[
            head.clone(),
            ps(b"Tab", 1),
            tail.clone(),
            ps(b"Arial", 1),
            ps(b"", 1),
        ]);
        let P::Text2(t) = one(opcode::TEXT2, &t11) else {
            panic!()
        };
        assert_eq!(
            (
                t.text.as_str(),
                t.raw,
                t.height,
                t.tracking,
                t.font_file.as_str()
            ),
            ("Tab", false, 2.0, 1.5, "Arial")
        );
        assert_eq!(
            (
                t.backwards,
                t.upside_down,
                t.vertical,
                t.underlined,
                t.overlined
            ),
            (true, false, true, false, true)
        );
        assert_eq!((t.font.is_none(), t.big_font_file.as_str()), (true, ""));

        let t38 = cat(&[
            head,
            pus("Täb"),
            tail,
            l(1),
            l(0),
            l(1),
            l(0x22),
            pus("Arial"),
            pus(""),
            pus("big"),
        ]);
        let P::Text2(t) = one(opcode::UNICODE_TEXT2, &t38) else {
            panic!()
        };
        assert_eq!((t.text.as_str(), t.big_font_file.as_str()), ("Täb", "big"));
        assert_eq!(
            t.font,
            Some(ProxyFont {
                bold: true,
                italic: false,
                charset: 1,
                pitch_and_family: 0x22,
                type_face: "Arial".into()
            })
        );
        assert_eq!(t.font_file, "");
    }

    #[test]
    fn state_opcodes() {
        assert_eq!(one(opcode::SUBENT_COLOR, &l(256)), P::SetColor(256));
        assert_eq!(one(opcode::SUBENT_LAYER, &l(4)), P::SetLayer(4));
        assert_eq!(one(opcode::SUBENT_LINETYPE, &l(-1)), P::SetLinetype(-1));
        assert_eq!(one(opcode::SUBENT_MARKER, &l(10)), P::SetMarker(10));
        assert_eq!(one(opcode::SUBENT_FILL_ON, &l(1)), P::SetFillMode(true));
        assert_eq!(one(opcode::SUBENT_FILL_ON, &l(0)), P::SetFillMode(false));
        assert_eq!(
            one(opcode::SUBENT_TRUE_COLOR, &0xC300_0007u32.to_le_bytes()),
            P::SetTrueColor {
                word: 0xC300_0007,
                color: Some(Color::Index(7))
            }
        );
        // The corpus's most common word is ByLayer, not RGB.
        for (word, want) in [
            // (u32 words)
            (0xC000_0000u32, Some(Color::ByLayer)),
            (0xC100_0000, Some(Color::ByBlock)),
            (
                0xC2FF_8001,
                Some(Color::Rgb {
                    r: 0xFF,
                    g: 0x80,
                    b: 0x01,
                }),
            ),
            (0xC800_0000, Some(Color::None)),
            (0x1234_5678, None),
        ] {
            assert_eq!(
                one(opcode::SUBENT_TRUE_COLOR, &word.to_le_bytes()),
                P::SetTrueColor { word, color: want },
                "{word:#X}"
            );
        }
        assert_eq!(one(opcode::SUBENT_LINEWEIGHT, &l(-3)), P::SetLineweight(-3));
        assert_eq!(one(opcode::SUBENT_LTSCALE, &f(2.5)), P::SetLtScale(2.5));
        assert_eq!(one(opcode::SUBENT_THICKNESS, &f(0.5)), P::SetThickness(0.5));
        assert_eq!(
            one(opcode::SUBENT_PLOT_STYLE, &cat(&[l(3), l(7)])),
            P::SetPlotStyle { kind: 3, index: 7 }
        );
        assert_eq!(
            one(opcode::SUBENT_MATERIAL, &0x1234u64.to_le_bytes()),
            P::SetMaterial(0x1234)
        );
        assert_eq!(
            one(
                opcode::SUBENT_MAPPER,
                &cat(&[l(0), l(0), l(1), l(2), l(3), l(4), l(0)])
            ),
            P::SetMapper {
                projection: 1,
                u_tiling: 2,
                v_tiling: 3,
                auto_transform: 4
            }
        );
        assert_eq!(one(opcode::POP_CLIP, &[]), P::PopClip);
        assert_eq!(one(opcode::POP_MODEL_XFORM, &[]), P::PopModelTransform);
    }

    #[test]
    fn transforms_and_clip() {
        let m: Vec<f64> = (0..16).map(f64::from).collect();
        let bytes: Vec<u8> = m.iter().flat_map(|&v| f(v)).collect();
        let want = P::PushModelTransform(std::array::from_fn(|i| i as f64));
        assert_eq!(one(opcode::PUSH_MODEL_XFORM, &bytes), want);
        // The second form's unknown trailing data is left out.
        let with_tail = cat(&[bytes.clone(), vec![1, 2, 3, 4]]);
        assert_eq!(one(opcode::PUSH_MODEL_XFORM2, &with_tail), want);

        let clip = cat(&[
            p3(A),
            p3(B),
            l(2),
            f(1.0),
            f(2.0),
            f(3.0),
            f(4.0),
            bytes.clone(),
            bytes,
            l(1),
            l(0),
            f(5.0),
            f(6.0),
            l(1),
        ]);
        let P::PushClip(c) = one(opcode::PUSH_CLIP, &clip) else {
            panic!()
        };
        assert_eq!(c.points, vec![[1.0, 2.0], [3.0, 4.0]]);
        assert_eq!((c.extrusion, c.origin), (A, B));
        assert_eq!(c.transform[15], 15.0);
        assert_eq!(c.inverse_block_transform[1], 1.0);
        assert_eq!(
            (
                c.front_clip_on,
                c.back_clip_on,
                c.front,
                c.back,
                c.draw_boundary
            ),
            (true, false, 5.0, 6.0, true)
        );
    }

    #[test]
    fn embedded_lwpolyline_decodes_with_the_entity_decoder() {
        let poly = crate::object::entities::poly::tests::lwpolyline();
        let mut w = ObjWriter::new(crate::Handle(1));
        poly.encode(&mut w).unwrap();
        let bits = w.data.into_bytes();
        // The data, then the 3 unknown bytes.
        let data = cat(&[l(bits.len() as i32), bits.clone(), vec![0; 3]]);
        let out = one(opcode::LWPOLYLINE, &data);
        assert_eq!(out, P::LwPolyline(bits));
        assert_eq!(out.lw_polyline(V).unwrap().unwrap(), poly);
        // Garbage embedded data is an error, not a panic; other primitives
        // have no polyline.
        assert!(P::LwPolyline(vec![0xFF]).lw_polyline(V).unwrap().is_err());
        assert!(P::PopClip.lw_polyline(V).is_none());
    }

    #[test]
    fn unknown_opcodes_are_kept_and_decoding_continues() {
        let b = blob(&[
            prim(51, &[1, 2, 3, 4]),
            prim(21, &[]),
            prim(0xABCD, &[9]),
            prim(opcode::SUBENT_COLOR, &l(1)),
        ]);
        assert_eq!(
            decode(&b, V).unwrap(),
            vec![
                P::Unknown {
                    opcode: 51,
                    data: vec![1, 2, 3, 4]
                },
                P::Unknown {
                    opcode: 21,
                    data: vec![]
                },
                P::Unknown {
                    opcode: 0xABCD,
                    data: vec![9]
                },
                P::SetColor(1)
            ]
        );
    }

    /// A primitive of every opcode `primitive` handles (the test
    /// `every_handled_opcode_has_a_fixture` checks that), so every record shape
    /// is cut: opcode, data and the bytes of the data its decoder needs (the
    /// rest is padding or data it leaves alone).
    fn rich_prims() -> Vec<(u32, Vec<u8>, usize)> {
        let head = cat(&[p3(A), p3(B), p3(C)]);
        let pts = cat(&[l(2), p3(A), p3(B)]);
        let tail = cat(&[
            l(3),
            l(1),
            f(2.0),
            f(0.8),
            f(0.1),
            f(1.5),
            l(1),
            l(0),
            l(1),
            l(0),
            l(1),
        ]);
        let m: Vec<u8> = (0..16).flat_map(|i| f(f64::from(i))).collect();
        let full = |op: u32, data: Vec<u8>| {
            let n = data.len();
            (op, data, n)
        };
        // The last string's padding is not needed.
        let minus = |op: u32, data: Vec<u8>, pad: usize| {
            let n = data.len() - pad;
            (op, data, n)
        };
        let text = |s: Vec<u8>| cat(&[head.clone(), f(1.0), f(1.0), f(0.0), s]);
        let t11 = cat(&[
            head.clone(),
            ps(b"Tab", 1),
            tail.clone(),
            ps(b"Arial", 1),
            ps(b"", 1),
        ]);
        let t38 = cat(&[
            head.clone(),
            pus("hi"),
            tail,
            l(0),
            l(0),
            l(1),
            l(0x22),
            pus("Arial"),
            pus(""),
            pus(""),
        ]);
        let clip = cat(&[
            p3(A),
            p3(B),
            l(1),
            f(1.0),
            f(2.0),
            m.clone(),
            m.clone(),
            l(1),
            l(0),
            f(5.0),
            f(6.0),
            l(1),
        ]);
        vec![
            full(opcode::EXTENTS, cat(&[p3(A), p3(B)])),
            full(opcode::CIRCLE, cat(&[p3(A), f(1.0), p3(B)])),
            full(opcode::CIRCLE_3PT, cat(&[p3(A), p3(B), p3(C)])),
            full(
                opcode::CIRCULAR_ARC,
                cat(&[p3(A), f(2.5), p3(B), p3(C), f(1.5), l(2)]),
            ),
            full(opcode::CIRCULAR_ARC_3PT, cat(&[p3(A), p3(B), p3(C), l(1)])),
            full(opcode::POLYLINE, pts.clone()),
            full(opcode::POLYGON, pts.clone()),
            full(opcode::MESH, cat(&[l(1), l(2), p3(A), p3(B)])),
            full(opcode::SHELL, cat(&[l(1), p3(A), l(2), l(1), l(0)])),
            minus(opcode::TEXT, text(ps(b"hi", 1)), 1),
            minus(opcode::TEXT2, t11, 3),
            full(opcode::XLINE, cat(&[p3(A), p3(B)])),
            full(opcode::RAY, cat(&[p3(A), p3(B)])),
            full(opcode::SUBENT_COLOR, l(1)),
            full(opcode::SUBENT_LAYER, l(1)),
            full(opcode::SUBENT_LINETYPE, l(1)),
            full(opcode::SUBENT_MARKER, l(1)),
            full(opcode::SUBENT_FILL_ON, l(1)),
            full(opcode::SUBENT_TRUE_COLOR, l(1)),
            full(opcode::SUBENT_LINEWEIGHT, l(1)),
            full(opcode::SUBENT_LTSCALE, f(1.0)),
            full(opcode::SUBENT_THICKNESS, f(1.0)),
            full(opcode::SUBENT_PLOT_STYLE, cat(&[l(3), l(1)])),
            full(opcode::PUSH_CLIP, clip),
            full(opcode::POP_CLIP, vec![]),
            full(opcode::PUSH_MODEL_XFORM, m.clone()),
            // The second form's trailing unknown data is not needed.
            (opcode::PUSH_MODEL_XFORM2, cat(&[m.clone(), l(7)]), 128),
            full(opcode::POP_MODEL_XFORM, vec![]),
            full(opcode::POLYLINE_WITH_NORMAL, cat(&[pts, p3(C)])),
            (opcode::LWPOLYLINE, cat(&[l(2), vec![1, 2], vec![0; 3]]), 6),
            full(opcode::SUBENT_MATERIAL, 7u64.to_le_bytes().to_vec()),
            // The mapper's seventh `RL` is not read.
            (
                opcode::SUBENT_MAPPER,
                cat(&[l(0), l(0), l(1), l(2), l(3), l(4), l(0)]),
                24,
            ),
            minus(opcode::UNICODE_TEXT, text(pus("hi")), 2),
            minus(opcode::UNICODE_TEXT2, t38, 2),
            (51, vec![0; 4], 0),
        ]
    }

    /// Every opcode `primitive` handles has a fixture in `rich_prims`: an
    /// opcode is handled when empty data is not decoded as `Unknown`. Opcodes
    /// above 64 are far past any `primitive` handles; a new one is caught
    /// where it is added to the `opcode` module (checked up to 255 here).
    #[test]
    fn every_handled_opcode_has_a_fixture() {
        let fixtures: Vec<u32> = rich_prims().iter().map(|(op, _, _)| *op).collect();
        for op in 0..=255 {
            let handled = !matches!(primitive(op, &[], 30), Ok(P::Unknown { .. }));
            assert_eq!(
                handled,
                fixtures.contains(&op) && op != 51,
                "opcode {op}: handled {handled}, fixture {}",
                fixtures.contains(&op)
            );
        }
    }

    fn rich_blob() -> Vec<u8> {
        let prims: Vec<Vec<u8>> = rich_prims().iter().map(|(op, d, _)| prim(*op, d)).collect();
        blob(&prims)
    }

    #[test]
    fn rich_blob_decodes() {
        assert_eq!(decode(&rich_blob(), V).unwrap().len(), rich_prims().len());
    }

    /// The blob with its total size set to its length, as a transfer cut at
    /// a record boundary or inside a record would leave a rewritten header.
    fn with_total(mut b: Vec<u8>) -> Vec<u8> {
        let n = b.len() as i32;
        b[..4].copy_from_slice(&l(n));
        b
    }

    #[test]
    fn truncation_at_every_byte_boundary_is_an_error() {
        let b = rich_blob();
        for n in 0..b.len() {
            assert!(decode(&b[..n], V).is_err(), "cut at {n}");
            // With the total size patched, the record loop reaches the cut:
            // the count (or a record size) still overruns the blob.
            if n >= 8 {
                assert!(
                    decode(&with_total(b[..n].to_vec()), V).is_err(),
                    "patched cut at {n}"
                );
            }
        }
    }

    /// A record cut short, inside a blob whose sizes are consistent, is
    /// `Undecoded` (or decodes, when only padding or ignored data is cut).
    #[test]
    fn record_level_truncation_is_undecoded() {
        let prims = rich_prims();
        for (at, (op, data, need)) in prims.iter().enumerate() {
            for k in 0..data.len() {
                let all: Vec<Vec<u8>> = prims
                    .iter()
                    .enumerate()
                    .map(|(i, (o, d, _))| prim(*o, if i == at { &d[..k] } else { d }))
                    .collect();
                let out = decode(&blob(&all), V).unwrap();
                assert_eq!(out.len(), prims.len(), "opcode {op} cut at {k}");
                match &out[at] {
                    P::Undecoded { opcode, data, .. } => {
                        assert_eq!((*opcode, data.len()), (*op, k));
                    }
                    other => assert!(k >= *need, "opcode {op} cut at {k}: {other:?}"),
                }
                if k < *need {
                    assert!(
                        matches!(out[at], P::Undecoded { .. }),
                        "opcode {op} cut at {k}"
                    );
                }
            }
        }
    }

    #[test]
    fn a_bad_record_does_not_hide_its_neighbours() {
        let b = blob(&[
            prim(opcode::SUBENT_COLOR, &l(1)),
            prim(opcode::CIRCLE, &[1, 2, 3]),
            prim(opcode::SUBENT_COLOR, &l(2)),
        ]);
        let out = decode(&b, V).unwrap();
        assert_eq!(out[0], P::SetColor(1));
        assert_eq!(out[2], P::SetColor(2));
        let P::Undecoded {
            opcode,
            data,
            error,
        } = &out[1]
        else {
            panic!("{:?}", out[1])
        };
        assert_eq!((*opcode, data.as_slice()), (opcode::CIRCLE, &[1, 2, 3][..]));
        assert!(error.contains("proxy graphics"), "{error}");
    }

    #[test]
    fn cur_count_checks_the_bytes_left() {
        let bytes = cat(&[l(2), vec![0; 24]]);
        assert!(Cur::new(&bytes).count(24).is_err());
        assert_eq!(Cur::new(&cat(&[l(1), vec![0; 24]])).count(24).unwrap(), 1);
        assert!(Cur::new(&l(-1)).count(1).is_err());
    }

    #[test]
    fn malformed_strings_are_flagged_lossy() {
        let head = cat(&[p3(A), p3(B), p3(C), f(1.0), f(1.0), f(0.0)]);
        // An unpaired high surrogate.
        let bad = [0x00, 0xD8, 0x41, 0x00];
        let data = cat(&[head.clone(), ps(&bad, 2)]);
        let P::Text { text, lossy, .. } = one(opcode::UNICODE_TEXT, &data) else {
            panic!()
        };
        assert_eq!((text.as_str(), lossy), ("\u{FFFD}A", true));
        // Malformed Shift-JIS (a lead byte at the end).
        let data = cat(&[head, ps(&[0x41, 0x81], 1)]);
        let out = decode_with_codepage(&blob(&[prim(opcode::TEXT, &data)]), V, 22).unwrap();
        assert!(matches!(&out[0], P::Text { lossy: true, .. }));
    }

    /// Flipping any byte of a blob never panics and never allocates wildly.
    #[test]
    fn corrupted_bytes_never_panic() {
        let b = rich_blob();
        for i in 0..b.len() {
            for v in [0x00, 0xFF, 0x7F, 0x80] {
                let mut bad = b.clone();
                bad[i] = v;
                let _ = decode(&bad, V);
            }
        }
    }

    #[test]
    fn hostile_sizes_and_counts() {
        let huge = u32::MAX as i32;
        // The count and the total size beyond the bytes.
        let mut b = blob(&[prim(opcode::SUBENT_COLOR, &l(1))]);
        b[4..8].copy_from_slice(&l(huge));
        assert!(decode(&b, V).is_err());
        let mut b = blob(&[prim(opcode::SUBENT_COLOR, &l(1))]);
        b[0..4].copy_from_slice(&l(huge));
        assert!(decode(&b, V).is_err());
        // A total below the header; a primitive smaller than its header or
        // larger than the blob.
        assert!(decode(&cat(&[l(4), l(0)]), V).is_err());
        let mut b = blob(&[prim(opcode::SUBENT_COLOR, &l(1))]);
        b[8..12].copy_from_slice(&l(4));
        assert!(matches!(decode(&b, V), Err(Error::Corrupt(_))));
        b[8..12].copy_from_slice(&l(huge));
        assert!(decode(&b, V).is_err());
        // Counts that the primitive cannot hold.
        for (op, data) in [
            (opcode::POLYLINE, l(huge)),
            (opcode::POLYGON, l(0x0100_0000)),
            (opcode::POLYLINE_WITH_NORMAL, l(huge)),
            (opcode::MESH, cat(&[l(huge), l(huge)])),
            (opcode::MESH, cat(&[l(65536), l(65536)])),
            (opcode::MESH, cat(&[l(0x4000_0000), l(0x4000_0000)])),
            (opcode::SHELL, l(huge)),
            (opcode::SHELL, cat(&[l(0), l(huge)])),
            (opcode::SHELL, cat(&[l(0), l(2), l(i32::MAX), l(0)])),
            (opcode::PUSH_CLIP, cat(&[p3(A), p3(B), l(huge)])),
            (opcode::LWPOLYLINE, l(huge)),
        ] {
            let out = decode(&blob(&[prim(op, &data)]), V).unwrap();
            assert!(
                matches!(&out[..], [P::Undecoded { opcode, error, .. }]
                    if *opcode == op && error.contains("truncated")),
                "opcode {op}: {out:?}"
            );
        }
        // Text without a terminator.
        let head = cat(&[p3(A), p3(B), p3(C), f(1.0), f(1.0), f(0.0)]);
        for (op, data) in [
            (opcode::TEXT, b"abcd".to_vec()),
            (opcode::UNICODE_TEXT, vec![0x41, 0, 0x42, 0]),
        ] {
            let b = blob(&[prim(op, &cat(&[head.clone(), data]))]);
            assert!(matches!(&decode(&b, V).unwrap()[..], [P::Undecoded { .. }]));
        }
    }

    /// Many empty primitives: the count is bounded by the blob, not trusted.
    #[test]
    fn many_primitives_are_bounded_by_the_blob() {
        let prims: Vec<Vec<u8>> = (0..10_000).map(|_| prim(opcode::POP_CLIP, &[])).collect();
        assert_eq!(decode(&blob(&prims), V).unwrap().len(), 10_000);
    }
}
