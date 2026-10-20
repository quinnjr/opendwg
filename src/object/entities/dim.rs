//! Dimensions and annotation (ODS 20.4.19 ARC_DIMENSION, 20.4.22-20.4.30
//! DIMENSION, 20.4.47 LEADER, 20.4.49 TOLERANCE). Multileaders are in
//! [`super::mleader`].
//!
//! A dimension's anonymous block (`*D...`) is a cached rendering of the
//! dimension: lines, arrowheads and the text AutoCAD drew for it. The block
//! reference is kept exactly as read and the block is never regenerated
//! (this library does no rendering), so after the dimension's geometry is
//! edited the block still shows the old drawing until AutoCAD regenerates it
//! (it does on opening the drawing and on REGEN). The format has no flag
//! asking for that: of the two bits ODS gives "Flags 1", bit 1 says the block
//! is used by this dimension only (DXF 70 bit 32), not that it is out of
//! date. The actual measurement, a stored copy of a value derived from the
//! geometry, is kept up to date instead (see [`Dimension::measurement`]).
//!
//! Other values AutoCAD derives when it lays an annotation out are cached
//! too, and likewise kept as read rather than recomputed after an edit: a
//! dimension's [`DimCommon::text_midpoint`] while the text is at its default
//! position; a LEADER's `endpoint_proj` and `block_offset` (and up to R2007
//! its `box_size`); a multileader's [`super::mleader::LeaderRoot::connection`],
//! [`super::mleader::MLeaderText`] boundary width and height, and
//! [`super::mleader::MLeaderBlock::matrix`]. AutoCAD recomputes them when it
//! regenerates the drawing.
//!
//! ARC_DIMENSION and LARGE_RADIAL_DIMENSION are class types, dispatched by
//! their class's DXF name.
//!
//! Where the corpus or ACadSharp (MIT) differ from ODS, each pinned by a test
//! in `tests/dimensions.rs`:
//! - DIMENSION "Flags 1" has bit 3 (8) set on every corpus dimension of
//!   every version (ODS: only the two lowest bits are used); its meaning is
//!   unknown and the byte is kept as read.
//! - From R2010 a LEADER stores no text box height and width (ODS: all
//!   versions), as ACadSharp `readLeader` reads it and the corpus confirms
//!   (the R2010+ leaders are consumed to the last bit without them).
//! - TOLERANCE text is a `TV` (ODS: `BS`), as ACadSharp `readTolerance`
//!   reads it.
//!
//! Unconfirmed by the corpus, which has none: ARC_DIMENSION and
//! LARGE_RADIAL_DIMENSION. They follow ODS.

use super::super::common::AsRead;
use super::super::stream::{ObjReader, ObjWriter};
use super::super::tables::{HARD_POINTER, encode_count, put};
use crate::error::{Error, Result};
use crate::handle::Handle;
use crate::version::Version;

pub(crate) fn corrupt(msg: impl Into<String>) -> Error {
    Error::Corrupt(msg.into())
}

/// A count read from the data stream, which must not be negative.
pub(crate) fn count(n: i32, what: &str) -> Result<usize> {
    usize::try_from(n).map_err(|_| corrupt(format!("negative {what} count {n}")))
}

/// Check that `n` items of at least `bits` bits each fit in the data stream
/// left, before allocating for them.
pub(crate) fn check_fits(r: &ObjReader, n: usize, bits: u64, context: &'static str) -> Result<()> {
    match (n as u64).checked_mul(bits) {
        Some(need) if need <= r.data.remaining_bits() => Ok(()),
        _ => Err(Error::Truncated { context }),
    }
}

/// The smallest encodings: a BD, BL or BS is 2 bits, a 3BD 6, a B 1.
pub(crate) const MIN_BD: u64 = 2;
pub(crate) const MIN_BL: u64 = 2;
pub(crate) const MIN_BS: u64 = 2;
pub(crate) const MIN_3BD: u64 = 6;

/// A `BL` count of items each taking at least `bits` data bits, checked
/// before anything is allocated for them.
pub(crate) fn read_count(r: &mut ObjReader, bits: u64, what: &'static str) -> Result<usize> {
    let n = count(r.data.read_bl()?, what)?;
    check_fits(r, n, bits, what)?;
    Ok(n)
}

/// Check that `n` references fit in the handle stream left (each takes at
/// least a byte).
pub(crate) fn check_handles(r: &ObjReader, n: usize, context: &'static str) -> Result<()> {
    if n as u64 <= r.handles.remaining_bits() / 8 {
        Ok(())
    } else {
        Err(Error::Truncated { context })
    }
}

pub(crate) fn write_count(w: &mut ObjWriter, n: usize, what: &str) -> Result<()> {
    w.data.write_bl(encode_count(n, what)?);
    Ok(())
}

pub(super) fn read_points(r: &mut ObjReader, what: &'static str) -> Result<Vec<[f64; 3]>> {
    let n = read_count(r, MIN_3BD, what)?;
    (0..n).map(|_| r.data.read_3bd()).collect()
}

pub(super) fn write_points(w: &mut ObjWriter, points: &[[f64; 3]], what: &str) -> Result<()> {
    write_count(w, points.len(), what)?;
    for &p in points {
        w.data.write_3bd(p);
    }
    Ok(())
}

/// Start and end point pairs (leader breaks).
pub(super) fn read_pairs(r: &mut ObjReader, what: &'static str) -> Result<Vec<[[f64; 3]; 2]>> {
    let n = read_count(r, 2 * MIN_3BD, what)?;
    (0..n)
        .map(|_| Ok([r.data.read_3bd()?, r.data.read_3bd()?]))
        .collect()
}

pub(super) fn write_pairs(w: &mut ObjWriter, pairs: &[[[f64; 3]; 2]], what: &str) -> Result<()> {
    write_count(w, pairs.len(), what)?;
    for [a, b] in pairs {
        w.data.write_3bd(*a);
        w.data.write_3bd(*b);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Dimensions.

/// The data every dimension starts with (ODS 20.4.22), and its handles.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct DimCommon {
    /// R2010+ class version (DXF 280); 0 before R2010.
    pub class_version: u8,
    pub extrusion: [f64; 3],
    /// The text middle point (DXF 11), in the OCS.
    pub text_midpoint: [f64; 2],
    /// The OCS Z of the text middle point and the clone insertion point.
    pub elevation: f64,
    /// ODS "Flags 1": bit 0 is set when the text is at its default position
    /// (the opposite of DXF 70 bit 128), bit 1 when the anonymous block is
    /// used by this dimension only (DXF 70 bit 32). Kept as read.
    pub flags: u8,
    /// The user text (DXF 1); empty for the measured value.
    pub user_text: String,
    /// DXF 53.
    pub text_rotation: f64,
    /// DXF 51: the negative of the angle between the OCS X axis and the UCS
    /// X axis (an ordinate dimension measures along the UCS axes).
    pub horiz_dir: f64,
    /// The anonymous block's insertion X, Y, Z scale (DXF 41-43).
    pub ins_scale: [f64; 3],
    /// The anonymous block's insertion rotation (DXF 54).
    pub ins_rotation: f64,
    /// DXF 71.
    pub attachment: i16,
    /// DXF 72.
    pub line_spacing_style: i16,
    /// DXF 41.
    pub line_spacing_factor: f64,
    /// The actual measurement (DXF 42) as stored. A copy derived from the
    /// geometry: see [`Dimension::measurement`] for the value written.
    pub measurement: f64,
    /// R2007+ unknown bit (DXF 73); `false` before R2007.
    pub unknown_73: bool,
    /// R2007+ flip arrow 1 and 2 (DXF 74, 75); `false` before R2007.
    pub flip_arrows: [bool; 2],
    /// The clone insertion point (DXF 12), in the OCS.
    pub clone_point: [f64; 2],
    /// The DIMSTYLE (hard pointer).
    pub dimstyle: Handle,
    /// The anonymous block (`*D...`, hard pointer): a cached rendering of
    /// the dimension, kept as read (see the module documentation).
    pub block: Handle,
}

/// The type-specific data of a dimension. Points are WCS unless noted.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub enum DimKind {
    /// ODS 20.4.23.
    Ordinate {
        /// The UCS origin the ordinate is measured from (DXF 10).
        def_point: [f64; 3],
        /// DXF 13.
        feature_location: [f64; 3],
        /// DXF 14.
        leader_endpoint: [f64; 3],
        /// ODS "Flags 2": [`ORDINATE_X`] set for an X ordinate (DXF 70 bit
        /// 64).
        flags2: u8,
    },
    /// ODS 20.4.24 (a rotated dimension).
    Linear {
        /// Extension line origins (DXF 13, 14).
        ext1: [f64; 3],
        ext2: [f64; 3],
        /// A point on the dimension line (DXF 10).
        def_point: [f64; 3],
        /// DXF 52.
        ext_line_rotation: f64,
        /// The OCS angle of the dimension line (DXF 50).
        rotation: f64,
    },
    /// ODS 20.4.25.
    Aligned {
        ext1: [f64; 3],
        ext2: [f64; 3],
        def_point: [f64; 3],
        ext_line_rotation: f64,
    },
    /// ODS 20.4.26.
    Angular3Pt {
        /// A point on the dimension arc (DXF 10).
        def_point: [f64; 3],
        /// Points on the two sides (DXF 13, 14).
        ext1: [f64; 3],
        ext2: [f64; 3],
        /// The angle vertex (DXF 15).
        vertex: [f64; 3],
    },
    /// ODS 20.4.27.
    Angular2Line {
        /// A point on the dimension arc (DXF 16), OCS X and Y.
        arc_point: [f64; 2],
        /// The first line (DXF 13 to 14).
        line1: [[f64; 3]; 2],
        /// The second line (DXF 15 to 10).
        line2: [[f64; 3]; 2],
    },
    /// ODS 20.4.28.
    Radius {
        /// The center (DXF 10).
        center: [f64; 3],
        /// The point on the curve (DXF 15).
        chord_point: [f64; 3],
        /// DXF 40.
        leader_length: f64,
    },
    /// ODS 20.4.29.
    Diameter {
        /// The point on the curve (DXF 15).
        chord_point: [f64; 3],
        /// The opposite point on the curve (DXF 10).
        far_chord_point: [f64; 3],
        leader_length: f64,
    },
    /// ARC_DIMENSION (ODS 20.4.19), a class type.
    ArcLength {
        /// A point on the dimension arc (DXF 10).
        arc_point: [f64; 3],
        /// DXF 13, 14.
        ext1: [f64; 3],
        ext2: [f64; 3],
        /// DXF 15.
        center: [f64; 3],
        /// DXF 70.
        is_partial: bool,
        /// DXF 40, 41 (radians).
        start_angle: f64,
        end_angle: f64,
        /// DXF 71.
        has_leader: bool,
        /// DXF 16, 17.
        leader1: [f64; 3],
        leader2: [f64; 3],
    },
    /// LARGE_RADIAL_DIMENSION (ODS 20.4.30), a class type.
    LargeRadial {
        /// DXF 10.
        center: [f64; 3],
        /// DXF 13.
        chord_point: [f64; 3],
        /// DXF 40 ("Value 0").
        unknown_40: f64,
        /// DXF 14.
        override_center: [f64; 3],
        /// DXF 15.
        jog_point: [f64; 3],
    },
}

/// Which dimension a type code or class names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DimType {
    Ordinate,
    Linear,
    Aligned,
    Angular3Pt,
    Angular2Line,
    Radius,
    Diameter,
    ArcLength,
    LargeRadial,
}

/// The geometry a measurement derives from, bit for bit.
type Geometry = Vec<u64>;

/// A dimension (ODS 20.4.22-20.4.30, 20.4.19).
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct Dimension {
    pub common: DimCommon,
    pub kind: DimKind,
    /// The geometry as read, to tell whether the stored measurement is
    /// still that of the geometry.
    #[cfg_attr(feature = "serde", serde(skip))]
    pub read: AsRead<Geometry>,
}

/// Transform a WCS point into the OCS of `extrusion` (the arbitrary axis
/// algorithm of the DXF reference). `None` for a zero extrusion.
fn to_ocs(p: [f64; 3], extrusion: [f64; 3]) -> Option<[f64; 3]> {
    let len = dot(extrusion, extrusion).sqrt();
    if len == 0.0 || !len.is_finite() {
        return None;
    }
    let n = extrusion.map(|v| v / len);
    let ax = if n[0].abs() < 1.0 / 64.0 && n[1].abs() < 1.0 / 64.0 {
        cross([0.0, 1.0, 0.0], n)
    } else {
        cross([0.0, 0.0, 1.0], n)
    };
    let ax_len = dot(ax, ax).sqrt();
    let ax = ax.map(|v| v / ax_len);
    let ay = cross(n, ax);
    Some([dot(p, ax), dot(p, ay), dot(p, n)])
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// The OCS points of `wcs`, when they all lie in one plane parallel to the
/// OCS XY plane (within rounding). Off that plane a dimension's measurement
/// depends on how AutoCAD projects, which the corpus does not show.
fn planar_ocs<const N: usize>(wcs: [[f64; 3]; N], extrusion: [f64; 3]) -> Option<[[f64; 2]; N]> {
    let mut out = [[0.0; 2]; N];
    let mut z0 = None;
    for (i, p) in wcs.into_iter().enumerate() {
        let [x, y, z] = to_ocs(p, extrusion)?;
        let scale = 1.0 + x.abs().max(y.abs()).max(z.abs());
        match z0 {
            None => z0 = Some(z),
            Some(z0) if (z - z0).abs() <= 1e-9 * scale => {}
            Some(_) => return None,
        }
        out[i] = [x, y];
    }
    Some(out)
}

fn sub2(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    [a[0] - b[0], a[1] - b[1]]
}

fn len2(v: [f64; 2]) -> f64 {
    v[0].hypot(v[1])
}

fn angle2(v: [f64; 2]) -> f64 {
    v[1].atan2(v[0])
}

const TAU: f64 = std::f64::consts::TAU;

/// The angle swept counterclockwise from `a` to `b`, in [0, 2π).
fn ccw(a: f64, b: f64) -> f64 {
    (b - a).rem_euclid(TAU)
}

/// The angle of the sector, bounded by consecutive rays of `rays` (angles),
/// that contains direction `d`.
fn sector(rays: &[f64], d: f64) -> f64 {
    // The ray just before `d` counterclockwise, and the one just after.
    let before = rays
        .iter()
        .map(|&r| ccw(r, d))
        .fold(f64::INFINITY, f64::min);
    let after = rays
        .iter()
        .map(|&r| ccw(d, r))
        .filter(|&a| a > 0.0)
        .fold(f64::INFINITY, f64::min);
    before + after
}

/// The angle swept around `v` from `a` to `b`, on the side that holds
/// `arc` (all OCS points): the angle between the rays or its complement.
fn sweep_through(arc: [f64; 2], a: [f64; 2], b: [f64; 2], v: [f64; 2]) -> Option<f64> {
    let (a, b, d) = (sub2(a, v), sub2(b, v), sub2(arc, v));
    if [a, b, d].iter().any(|&x| len2(x) == 0.0) {
        return None;
    }
    let sweep = ccw(angle2(a), angle2(b));
    Some(if ccw(angle2(a), angle2(d)) <= sweep {
        sweep
    } else {
        TAU - sweep
    })
}

/// Ordinate "Flags 2": an X ordinate (DXF 70 bit 64); clear for Y.
pub const ORDINATE_X: u8 = 1;

impl DimKind {
    /// The measurement computed from the geometry (taken in the OCS of
    /// `extrusion`, by the arbitrary axis algorithm):
    /// - linear: the distance along the dimension line's rotation;
    /// - aligned: the distance between the extension line origins;
    /// - angular: the angle of the sector that holds the dimension arc
    ///   point (3-point: around the vertex; 2-line: between the four rays
    ///   from the lines' intersection);
    /// - radius and large radial: the distance from the center to the chord
    ///   point; diameter: between the two chord points;
    /// - arc length: radius (center to ext 1) times the sweep from ext 1 to
    ///   ext 2 around the center that holds the arc point;
    /// - ordinate: the distance from the definition point (the UCS origin)
    ///   to the feature along the UCS X axis (flags 2 bit 0 set) or Y axis.
    ///   DXF 51 `horiz_dir` is "the negative of the angle between the OCS X
    ///   axis and the UCS X axis", so the UCS X axis is the OCS X axis
    ///   rotated by `-horiz_dir`.
    ///
    /// `None` for degenerate geometry (zero radius, parallel lines, an arc
    /// point at the vertex) and for points not in one plane parallel to the
    /// OCS XY plane, where the stored value is kept.
    ///
    /// The value is the geometric one, not scaled by DIMLFAC. Whether
    /// AutoCAD's stored value (DXF 42) is scaled is not said by ODS ("Actual
    /// Measurement") or the DXF reference ("Actual measurement (optional;
    /// read-only value)"), and the corpus dimension styles all keep DIMLFAC
    /// at 1, so it is unconfirmed.
    pub fn computed_measurement(&self, extrusion: [f64; 3], horiz_dir: f64) -> Option<f64> {
        let m = match *self {
            DimKind::Linear {
                ext1,
                ext2,
                rotation,
                ..
            } => {
                let [a, b] = planar_ocs([ext1, ext2], extrusion)?;
                let v = sub2(b, a);
                (v[0] * rotation.cos() + v[1] * rotation.sin()).abs()
            }
            DimKind::Aligned { ext1, ext2, .. } => {
                let [a, b] = planar_ocs([ext1, ext2], extrusion)?;
                len2(sub2(b, a))
            }
            DimKind::Radius {
                center,
                chord_point,
                ..
            }
            | DimKind::LargeRadial {
                center,
                chord_point,
                ..
            } => {
                let [a, b] = planar_ocs([center, chord_point], extrusion)?;
                len2(sub2(b, a))
            }
            DimKind::Diameter {
                chord_point,
                far_chord_point,
                ..
            } => {
                let [a, b] = planar_ocs([chord_point, far_chord_point], extrusion)?;
                len2(sub2(b, a))
            }
            DimKind::Angular3Pt {
                def_point,
                ext1,
                ext2,
                vertex,
            } => {
                let [arc, a, b, v] = planar_ocs([def_point, ext1, ext2, vertex], extrusion)?;
                sweep_through(arc, a, b, v)?
            }
            DimKind::ArcLength {
                arc_point,
                ext1,
                ext2,
                center,
                ..
            } => {
                let [arc, a, b, c] = planar_ocs([arc_point, ext1, ext2, center], extrusion)?;
                let radius = len2(sub2(a, c));
                if radius == 0.0 {
                    return None;
                }
                radius * sweep_through(arc, a, b, c)?
            }
            DimKind::Angular2Line {
                arc_point,
                line1,
                line2,
            } => {
                let [p1, p2, q1, q2] =
                    planar_ocs([line1[0], line1[1], line2[0], line2[1]], extrusion)?;
                let (u, v) = (sub2(p2, p1), sub2(q2, q1));
                let det = u[0] * v[1] - u[1] * v[0];
                if det == 0.0 || len2(u) == 0.0 || len2(v) == 0.0 {
                    return None;
                }
                // The lines' intersection.
                let w = sub2(q1, p1);
                let t = (w[0] * v[1] - w[1] * v[0]) / det;
                let x = [p1[0] + t * u[0], p1[1] + t * u[1]];
                let d = sub2(arc_point, x);
                if len2(d) == 0.0 {
                    return None;
                }
                let (a, b) = (angle2(u), angle2(v));
                let half = std::f64::consts::PI;
                sector(&[a, a + half, b, b + half], angle2(d))
            }
            DimKind::Ordinate {
                def_point,
                feature_location,
                flags2,
                ..
            } => {
                let [o, f] = planar_ocs([def_point, feature_location], extrusion)?;
                let d = sub2(f, o);
                // The UCS X axis in the OCS, and its Y axis.
                let (sin, cos) = (-horiz_dir).sin_cos();
                let axis = if flags2 & ORDINATE_X != 0 {
                    [cos, sin]
                } else {
                    [-sin, cos]
                };
                (d[0] * axis[0] + d[1] * axis[1]).abs()
            }
        };
        m.is_finite().then_some(m)
    }

    fn dim_type(&self) -> DimType {
        match self {
            DimKind::Ordinate { .. } => DimType::Ordinate,
            DimKind::Linear { .. } => DimType::Linear,
            DimKind::Aligned { .. } => DimType::Aligned,
            DimKind::Angular3Pt { .. } => DimType::Angular3Pt,
            DimKind::Angular2Line { .. } => DimType::Angular2Line,
            DimKind::Radius { .. } => DimType::Radius,
            DimKind::Diameter { .. } => DimType::Diameter,
            DimKind::ArcLength { .. } => DimType::ArcLength,
            DimKind::LargeRadial { .. } => DimType::LargeRadial,
        }
    }

    /// The fixed type code, or the class DXF name of a class type.
    pub(crate) fn type_key(&self) -> std::result::Result<u16, &'static str> {
        use super::super::type_code::*;
        Ok(match self.dim_type() {
            DimType::Ordinate => DIM_ORDINATE,
            DimType::Linear => DIM_LINEAR,
            DimType::Aligned => DIM_ALIGNED,
            DimType::Angular3Pt => DIM_ANG3PT,
            DimType::Angular2Line => DIM_ANG2LN,
            DimType::Radius => DIM_RADIUS,
            DimType::Diameter => DIM_DIAMETER,
            DimType::ArcLength => return Err("ARC_DIMENSION"),
            DimType::LargeRadial => return Err("LARGE_RADIAL_DIMENSION"),
        })
    }

    /// Every value the measurement derives from, bit for bit: the kind's
    /// values, the extrusion, and for an ordinate the horizontal direction
    /// (which gives the UCS axes).
    fn geometry(&self, extrusion: [f64; 3], horiz_dir: f64) -> Geometry {
        let mut g: Vec<u64> = extrusion.iter().map(|v| v.to_bits()).collect();
        let mut p = |v: &[f64]| g.extend(v.iter().map(|x| x.to_bits()));
        match self {
            DimKind::Ordinate {
                def_point,
                feature_location,
                leader_endpoint,
                flags2,
            } => {
                p(def_point);
                p(feature_location);
                p(leader_endpoint);
                p(&[f64::from(*flags2), horiz_dir]);
            }
            DimKind::Linear {
                ext1,
                ext2,
                def_point,
                ext_line_rotation,
                rotation,
            } => {
                p(ext1);
                p(ext2);
                p(def_point);
                p(&[*ext_line_rotation, *rotation]);
            }
            DimKind::Aligned {
                ext1,
                ext2,
                def_point,
                ext_line_rotation,
            } => {
                p(ext1);
                p(ext2);
                p(def_point);
                p(&[*ext_line_rotation]);
            }
            DimKind::Angular3Pt {
                def_point,
                ext1,
                ext2,
                vertex,
            } => {
                p(def_point);
                p(ext1);
                p(ext2);
                p(vertex);
            }
            DimKind::Angular2Line {
                arc_point,
                line1,
                line2,
            } => {
                p(arc_point);
                p(line1.as_flattened());
                p(line2.as_flattened());
            }
            DimKind::Radius {
                center,
                chord_point,
                leader_length,
            } => {
                p(center);
                p(chord_point);
                p(&[*leader_length]);
            }
            DimKind::Diameter {
                chord_point,
                far_chord_point,
                leader_length,
            } => {
                p(chord_point);
                p(far_chord_point);
                p(&[*leader_length]);
            }
            DimKind::ArcLength {
                arc_point,
                ext1,
                ext2,
                center,
                is_partial,
                start_angle,
                end_angle,
                has_leader,
                leader1,
                leader2,
            } => {
                p(arc_point);
                p(ext1);
                p(ext2);
                p(center);
                p(&[f64::from(u8::from(*is_partial)), *start_angle, *end_angle]);
                p(&[f64::from(u8::from(*has_leader))]);
                p(leader1);
                p(leader2);
            }
            DimKind::LargeRadial {
                center,
                chord_point,
                unknown_40,
                override_center,
                jog_point,
            } => {
                p(center);
                p(chord_point);
                p(&[*unknown_40]);
                p(override_center);
                p(jog_point);
            }
        }
        g
    }

    fn decode(r: &mut ObjReader, t: DimType) -> Result<Self> {
        let d = &mut r.data;
        Ok(match t {
            DimType::Ordinate => DimKind::Ordinate {
                def_point: d.read_3bd()?,
                feature_location: d.read_3bd()?,
                leader_endpoint: d.read_3bd()?,
                flags2: d.read_rc()?,
            },
            DimType::Linear => DimKind::Linear {
                ext1: d.read_3bd()?,
                ext2: d.read_3bd()?,
                def_point: d.read_3bd()?,
                ext_line_rotation: d.read_bd()?,
                rotation: d.read_bd()?,
            },
            DimType::Aligned => DimKind::Aligned {
                ext1: d.read_3bd()?,
                ext2: d.read_3bd()?,
                def_point: d.read_3bd()?,
                ext_line_rotation: d.read_bd()?,
            },
            DimType::Angular3Pt => DimKind::Angular3Pt {
                def_point: d.read_3bd()?,
                ext1: d.read_3bd()?,
                ext2: d.read_3bd()?,
                vertex: d.read_3bd()?,
            },
            DimType::Angular2Line => {
                let arc_point = d.read_2rd()?;
                let l1 = [d.read_3bd()?, d.read_3bd()?];
                let (s2, e2) = (d.read_3bd()?, d.read_3bd()?);
                DimKind::Angular2Line {
                    arc_point,
                    line1: l1,
                    line2: [s2, e2],
                }
            }
            DimType::Radius => DimKind::Radius {
                center: d.read_3bd()?,
                chord_point: d.read_3bd()?,
                leader_length: d.read_bd()?,
            },
            DimType::Diameter => DimKind::Diameter {
                chord_point: d.read_3bd()?,
                far_chord_point: d.read_3bd()?,
                leader_length: d.read_bd()?,
            },
            DimType::ArcLength => DimKind::ArcLength {
                arc_point: d.read_3bd()?,
                ext1: d.read_3bd()?,
                ext2: d.read_3bd()?,
                center: d.read_3bd()?,
                is_partial: d.read_b()?,
                start_angle: d.read_bd()?,
                end_angle: d.read_bd()?,
                has_leader: d.read_b()?,
                leader1: d.read_3bd()?,
                leader2: d.read_3bd()?,
            },
            DimType::LargeRadial => DimKind::LargeRadial {
                center: d.read_3bd()?,
                chord_point: d.read_3bd()?,
                unknown_40: d.read_bd()?,
                override_center: d.read_3bd()?,
                jog_point: d.read_3bd()?,
            },
        })
    }

    fn encode(&self, w: &mut ObjWriter) {
        let d = &mut w.data;
        match *self {
            DimKind::Ordinate {
                def_point,
                feature_location,
                leader_endpoint,
                flags2,
            } => {
                d.write_3bd(def_point);
                d.write_3bd(feature_location);
                d.write_3bd(leader_endpoint);
                d.write_rc(flags2);
            }
            DimKind::Linear {
                ext1,
                ext2,
                def_point,
                ext_line_rotation,
                rotation,
            } => {
                d.write_3bd(ext1);
                d.write_3bd(ext2);
                d.write_3bd(def_point);
                d.write_bd(ext_line_rotation);
                d.write_bd(rotation);
            }
            DimKind::Aligned {
                ext1,
                ext2,
                def_point,
                ext_line_rotation,
            } => {
                d.write_3bd(ext1);
                d.write_3bd(ext2);
                d.write_3bd(def_point);
                d.write_bd(ext_line_rotation);
            }
            DimKind::Angular3Pt {
                def_point,
                ext1,
                ext2,
                vertex,
            } => {
                for p in [def_point, ext1, ext2, vertex] {
                    d.write_3bd(p);
                }
            }
            DimKind::Angular2Line {
                arc_point,
                line1,
                line2,
            } => {
                d.write_2rd(arc_point);
                for p in [line1[0], line1[1], line2[0], line2[1]] {
                    d.write_3bd(p);
                }
            }
            DimKind::Radius {
                center: a,
                chord_point: b,
                leader_length,
            }
            | DimKind::Diameter {
                chord_point: a,
                far_chord_point: b,
                leader_length,
            } => {
                d.write_3bd(a);
                d.write_3bd(b);
                d.write_bd(leader_length);
            }
            DimKind::ArcLength {
                arc_point,
                ext1,
                ext2,
                center,
                is_partial,
                start_angle,
                end_angle,
                has_leader,
                leader1,
                leader2,
            } => {
                for p in [arc_point, ext1, ext2, center] {
                    d.write_3bd(p);
                }
                d.write_b(is_partial);
                d.write_bd(start_angle);
                d.write_bd(end_angle);
                d.write_b(has_leader);
                d.write_3bd(leader1);
                d.write_3bd(leader2);
            }
            DimKind::LargeRadial {
                center,
                chord_point,
                unknown_40,
                override_center,
                jog_point,
            } => {
                d.write_3bd(center);
                d.write_3bd(chord_point);
                d.write_bd(unknown_40);
                d.write_3bd(override_center);
                d.write_3bd(jog_point);
            }
        }
    }
}

impl DimCommon {
    /// The data fields; the handles come after the type-specific data.
    fn decode(r: &mut ObjReader) -> Result<Self> {
        let v = r.version;
        let class_version = if v >= Version::R2010 {
            r.data.read_rc()?
        } else {
            0
        };
        let d = &mut r.data;
        let extrusion = d.read_3bd()?;
        let text_midpoint = d.read_2rd()?;
        let elevation = d.read_bd()?;
        let flags = d.read_rc()?;
        let user_text = r.text()?;
        let d = &mut r.data;
        let text_rotation = d.read_bd()?;
        let horiz_dir = d.read_bd()?;
        let ins_scale = [d.read_bd()?, d.read_bd()?, d.read_bd()?];
        let ins_rotation = d.read_bd()?;
        // R2000+ (every version read).
        let attachment = d.read_bs()?;
        let line_spacing_style = d.read_bs()?;
        let line_spacing_factor = d.read_bd()?;
        let measurement = d.read_bd()?;
        let (unknown_73, flip_arrows) = if v >= Version::R2007 {
            (d.read_b()?, [d.read_b()?, d.read_b()?])
        } else {
            (false, [false; 2])
        };
        let clone_point = d.read_2rd()?;
        Ok(DimCommon {
            class_version,
            extrusion,
            text_midpoint,
            elevation,
            flags,
            user_text,
            text_rotation,
            horiz_dir,
            ins_scale,
            ins_rotation,
            attachment,
            line_spacing_style,
            line_spacing_factor,
            measurement,
            unknown_73,
            flip_arrows,
            clone_point,
            dimstyle: Handle(0),
            block: Handle(0),
        })
    }

    fn encode(&self, w: &mut ObjWriter, measurement: f64) -> Result<()> {
        w.data.write_rc(self.class_version);
        let d = &mut w.data;
        d.write_3bd(self.extrusion);
        d.write_2rd(self.text_midpoint);
        d.write_bd(self.elevation);
        d.write_rc(self.flags);
        w.text(&self.user_text)?;
        let d = &mut w.data;
        d.write_bd(self.text_rotation);
        d.write_bd(self.horiz_dir);
        for s in self.ins_scale {
            d.write_bd(s);
        }
        d.write_bd(self.ins_rotation);
        d.write_bs(self.attachment);
        d.write_bs(self.line_spacing_style);
        d.write_bd(self.line_spacing_factor);
        d.write_bd(measurement);
        d.write_b(self.unknown_73);
        d.write_b(self.flip_arrows[0]);
        d.write_b(self.flip_arrows[1]);
        d.write_2rd(self.clone_point);
        Ok(())
    }
}

impl Dimension {
    pub(crate) fn decode(r: &mut ObjReader, t: DimType) -> Result<Self> {
        let mut common = DimCommon::decode(r)?;
        let kind = DimKind::decode(r, t)?;
        common.dimstyle = r.href_abs()?;
        common.block = r.href_abs()?;
        let read = AsRead::new(kind.geometry(common.extrusion, common.horiz_dir));
        Ok(Dimension { common, kind, read })
    }

    /// The actual measurement to write (DXF 42). While the geometry (the
    /// type-specific values, the extrusion and an ordinate's horizontal
    /// direction) is unchanged since it was read, bit for bit, the stored
    /// [`DimCommon::measurement`]; otherwise, and for a dimension built by
    /// hand, the value computed from the geometry
    /// ([`DimKind::computed_measurement`]), or the stored field where the
    /// geometry is degenerate or not planar.
    pub fn measurement(&self) -> f64 {
        let c = &self.common;
        let unchanged = self
            .read
            .get()
            .is_some_and(|g| *g == self.kind.geometry(c.extrusion, c.horiz_dir));
        if unchanged {
            return c.measurement;
        }
        self.kind
            .computed_measurement(c.extrusion, c.horiz_dir)
            .unwrap_or(c.measurement)
    }

    pub(crate) fn encode(&self, w: &mut ObjWriter) -> Result<()> {
        self.common.encode(w, self.measurement())?;
        self.kind.encode(w);
        put(w, HARD_POINTER, self.common.dimstyle)?;
        put(w, HARD_POINTER, self.common.block)
    }
}

// ---------------------------------------------------------------------------
// LEADER and TOLERANCE.

/// A leader (ODS 20.4.47).
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct Leader {
    /// "Always seems to be 0".
    pub unknown_bit: bool,
    /// The annotation type: 0 MTEXT, 1 TOLERANCE, 2 INSERT, 3 none (DXF 73).
    pub annot_type: i16,
    /// 0 straight, 1 spline (DXF 72).
    pub path_type: i16,
    /// The vertices (DXF 10).
    pub points: Vec<[f64; 3]>,
    /// The leader plane origin (by default the first point).
    pub origin: [f64; 3],
    pub extrusion: [f64; 3],
    /// DXF 211.
    pub x_direction: [f64; 3],
    /// The block offset (DXF 212).
    pub block_offset: [f64; 3],
    /// The offset from the last point to the annotation (DXF 213).
    pub endpoint_proj: [f64; 3],
    /// Up to R2007: the annotation's text box height and width (DXF 40,
    /// 41). From R2010 files do not store them (ODS: all versions; as
    /// ACadSharp `readLeader` reads them), so they are `None` and an R2018
    /// write leaves them out.
    pub box_size: Option<[f64; 2]>,
    /// The hook line is on the X direction (DXF 74).
    pub hook_line_on_x_dir: bool,
    /// DXF 71.
    pub arrowhead_on: bool,
    /// R2000+ unknown fields.
    pub unknown_bs: i16,
    pub unknown_b: [bool; 2],
    /// The associated annotation (DXF 340, hard pointer).
    pub annotation: Handle,
    /// The DIMSTYLE (hard pointer).
    pub dimstyle: Handle,
}

impl Leader {
    pub(crate) fn decode(r: &mut ObjReader) -> Result<Self> {
        let unknown_bit = r.data.read_b()?;
        let annot_type = r.data.read_bs()?;
        let path_type = r.data.read_bs()?;
        let points = read_points(r, "leader points")?;
        let d = &mut r.data;
        Ok(Leader {
            unknown_bit,
            annot_type,
            path_type,
            points,
            origin: d.read_3bd()?,
            extrusion: d.read_3bd()?,
            x_direction: d.read_3bd()?,
            block_offset: d.read_3bd()?,
            endpoint_proj: d.read_3bd()?,
            box_size: if r.version <= Version::R2007 {
                Some([d.read_bd()?, d.read_bd()?])
            } else {
                None
            },
            hook_line_on_x_dir: d.read_b()?,
            arrowhead_on: d.read_b()?,
            unknown_bs: d.read_bs()?,
            unknown_b: [d.read_b()?, d.read_b()?],
            annotation: r.href_abs()?,
            dimstyle: r.href_abs()?,
        })
    }

    pub(crate) fn encode(&self, w: &mut ObjWriter) -> Result<()> {
        w.data.write_b(self.unknown_bit);
        w.data.write_bs(self.annot_type);
        w.data.write_bs(self.path_type);
        write_points(w, &self.points, "leader points")?;
        let d = &mut w.data;
        for p in [
            self.origin,
            self.extrusion,
            self.x_direction,
            self.block_offset,
            self.endpoint_proj,
        ] {
            d.write_3bd(p);
        }
        d.write_b(self.hook_line_on_x_dir);
        d.write_b(self.arrowhead_on);
        d.write_bs(self.unknown_bs);
        d.write_b(self.unknown_b[0]);
        d.write_b(self.unknown_b[1]);
        put(w, HARD_POINTER, self.annotation)?;
        put(w, HARD_POINTER, self.dimstyle)
    }
}

/// A geometric tolerance (ODS 20.4.49).
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct Tolerance {
    pub insertion: [f64; 3],
    /// DXF 11.
    pub x_direction: [f64; 3],
    pub extrusion: [f64; 3],
    /// The tolerance text with its format codes (DXF 1).
    pub text: String,
    /// The DIMSTYLE (hard pointer).
    pub dimstyle: Handle,
}

impl Tolerance {
    pub(crate) fn decode(r: &mut ObjReader) -> Result<Self> {
        let d = &mut r.data;
        let insertion = d.read_3bd()?;
        let x_direction = d.read_3bd()?;
        let extrusion = d.read_3bd()?;
        Ok(Tolerance {
            insertion,
            x_direction,
            extrusion,
            text: r.text()?,
            dimstyle: r.href_abs()?,
        })
    }

    pub(crate) fn encode(&self, w: &mut ObjWriter) -> Result<()> {
        w.data.write_3bd(self.insertion);
        w.data.write_3bd(self.x_direction);
        w.data.write_3bd(self.extrusion);
        w.text(&self.text)?;
        put(w, HARD_POINTER, self.dimstyle)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::super::tests::{reader, roundtrip};
    use super::*;
    use crate::bits::BitWriter;
    use crate::handle::HandleRef;
    use crate::object::stream::tests::assemble;
    use crate::object::type_code;

    fn common() -> DimCommon {
        DimCommon {
            class_version: 1,
            extrusion: [0.0, 0.0, 1.0],
            text_midpoint: [5.0, 6.0],
            elevation: 2.5,
            flags: 11,
            user_text: "<> mm".into(),
            text_rotation: 0.25,
            horiz_dir: -0.5,
            ins_scale: [1.0, 2.0, 3.0],
            ins_rotation: 0.125,
            attachment: 5,
            line_spacing_style: 1,
            line_spacing_factor: 1.5,
            measurement: 3.0,
            unknown_73: true,
            flip_arrows: [true, false],
            clone_point: [7.0, -8.0],
            dimstyle: Handle(0x27),
            block: Handle(0x1F3),
        }
    }

    const P: [f64; 3] = [1.0, 2.0, 0.0];
    const Q: [f64; 3] = [4.0, 6.0, 0.0];
    const R: [f64; 3] = [-3.0, 0.5, 0.0];

    pub(crate) fn kinds() -> Vec<DimKind> {
        vec![
            DimKind::Ordinate {
                def_point: P,
                feature_location: Q,
                leader_endpoint: R,
                flags2: 1,
            },
            DimKind::Linear {
                ext1: P,
                ext2: Q,
                def_point: R,
                ext_line_rotation: 0.5,
                rotation: 0.25,
            },
            DimKind::Aligned {
                ext1: P,
                ext2: Q,
                def_point: R,
                ext_line_rotation: -0.5,
            },
            DimKind::Angular3Pt {
                def_point: R,
                ext1: P,
                ext2: Q,
                vertex: [0.0; 3],
            },
            DimKind::Angular2Line {
                arc_point: [9.0, 10.0],
                line1: [P, Q],
                line2: [R, [0.0; 3]],
            },
            DimKind::Radius {
                center: P,
                chord_point: Q,
                leader_length: 2.0,
            },
            DimKind::Diameter {
                chord_point: P,
                far_chord_point: Q,
                leader_length: 0.0,
            },
            DimKind::ArcLength {
                arc_point: R,
                ext1: P,
                ext2: Q,
                center: [0.0; 3],
                is_partial: true,
                start_angle: 0.25,
                end_angle: 1.5,
                has_leader: true,
                leader1: [1.0; 3],
                leader2: [2.0; 3],
            },
            DimKind::LargeRadial {
                center: P,
                chord_point: Q,
                unknown_40: 0.0,
                override_center: R,
                jog_point: [3.0; 3],
            },
        ]
    }

    /// A dimension as built by hand (nothing read).
    pub(crate) fn dimension(kind: DimKind) -> Dimension {
        Dimension {
            common: common(),
            kind,
            read: AsRead::default(),
        }
    }

    /// A hand-built dimension whose stored measurement is the one written.
    pub(crate) fn measured(kind: DimKind) -> Dimension {
        let mut d = dimension(kind);
        d.common.measurement = d.measurement();
        d
    }

    fn dim_code(d: &Dimension) -> u16 {
        // A class type is written with some class number; any will do here.
        d.kind.type_key().unwrap_or(500)
    }

    /// Encode a dimension as R2018 and read it back (so its geometry and
    /// measurement are "as read").
    fn reread(d: &Dimension) -> Dimension {
        let mut w = ObjWriter::new(Handle(0x80));
        d.encode(&mut w).unwrap();
        let bytes = w.finish(dim_code(d)).unwrap();
        Dimension::decode(&mut reader(&bytes), d.kind.dim_type()).unwrap()
    }

    #[test]
    fn roundtrip_dimensions() {
        for kind in kinds() {
            let t = kind.dim_type();
            // The written measurement is computed where there is a formula,
            // so a round trip starts from that value.
            let mut d = dimension(kind);
            d.common.measurement = d.measurement();
            let code = dim_code(&d);
            let dec = match t {
                DimType::Ordinate => |r: &mut ObjReader| Dimension::decode(r, DimType::Ordinate),
                DimType::Linear => |r: &mut ObjReader| Dimension::decode(r, DimType::Linear),
                DimType::Aligned => |r: &mut ObjReader| Dimension::decode(r, DimType::Aligned),
                DimType::Angular3Pt => {
                    |r: &mut ObjReader| Dimension::decode(r, DimType::Angular3Pt)
                }
                DimType::Angular2Line => {
                    |r: &mut ObjReader| Dimension::decode(r, DimType::Angular2Line)
                }
                DimType::Radius => |r: &mut ObjReader| Dimension::decode(r, DimType::Radius),
                DimType::Diameter => |r: &mut ObjReader| Dimension::decode(r, DimType::Diameter),
                DimType::ArcLength => |r: &mut ObjReader| Dimension::decode(r, DimType::ArcLength),
                DimType::LargeRadial => {
                    |r: &mut ObjReader| Dimension::decode(r, DimType::LargeRadial)
                }
            };
            roundtrip(&d, Dimension::encode, dec, code);
        }
    }

    #[test]
    fn dimension_type_keys() {
        let keys: Vec<_> = kinds().iter().map(DimKind::type_key).collect();
        assert_eq!(
            keys,
            [
                Ok(type_code::DIM_ORDINATE),
                Ok(type_code::DIM_LINEAR),
                Ok(type_code::DIM_ALIGNED),
                Ok(type_code::DIM_ANG3PT),
                Ok(type_code::DIM_ANG2LN),
                Ok(type_code::DIM_RADIUS),
                Ok(type_code::DIM_DIAMETER),
                Err("ARC_DIMENSION"),
                Err("LARGE_RADIAL_DIMENSION"),
            ]
        );
    }

    /// An R2004 dimension: no class version, no R2007 bits, its text in the
    /// data stream; the missing values read as their defaults.
    #[test]
    fn r2004_dimension() {
        let mut d = BitWriter::new();
        d.write_3bd([0.0, 0.0, 1.0]);
        d.write_2rd([5.0, 6.0]);
        d.write_bd(0.0);
        d.write_rc(11);
        d.write_tv_raw(b"abc").unwrap();
        for v in [0.0, 0.0, 1.0, 1.0, 1.0, 0.0] {
            d.write_bd(v);
        }
        d.write_bs(5);
        d.write_bs(1);
        d.write_bd(1.0);
        d.write_bd(5.0); // measurement
        d.write_2rd([0.0, 0.0]);
        // Radius: center, chord point, leader length.
        d.write_3bd([0.0; 3]);
        d.write_3bd([3.0, 4.0, 0.0]);
        d.write_bd(0.0);
        let mut h = BitWriter::new();
        for value in [0x27, 0x1F3] {
            h.write_handle_ref(HandleRef { code: 5, value }).unwrap();
        }
        let bytes = assemble(Version::R2004, type_code::DIM_RADIUS, &d, None, &h);
        let (mut r, _) = ObjReader::split(&bytes, Version::R2004, 30).unwrap();
        let x = Dimension::decode(&mut r, DimType::Radius).unwrap();
        assert_eq!(r.data.remaining_bits(), 0);
        let c = &x.common;
        assert_eq!(
            (c.class_version, c.unknown_73, c.flip_arrows),
            (0, false, [false; 2])
        );
        assert_eq!((c.user_text.as_str(), c.measurement), ("abc", 5.0));
        assert_eq!((c.dimstyle, c.block), (Handle(0x27), Handle(0x1F3)));
        assert_eq!(
            x.kind.computed_measurement(c.extrusion, c.horiz_dir),
            Some(5.0)
        );
        // As R2018 it reads back equal, with the stored measurement.
        assert_eq!(reread(&x), x);
        assert_eq!(reread(&x).common.measurement, 5.0);
    }

    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-12
    }

    #[test]
    fn measurement_formulas() {
        let z = [0.0, 0.0, 1.0];
        let m = |k: DimKind| k.computed_measurement(z, 0.0);
        // Linear: along the rotation; aligned: the distance.
        let lin = |rotation| DimKind::Linear {
            ext1: P,
            ext2: Q,
            def_point: R,
            ext_line_rotation: 0.0,
            rotation,
        };
        assert!(approx(m(lin(0.0)).unwrap(), 3.0));
        assert!(approx(m(lin(std::f64::consts::FRAC_PI_2)).unwrap(), 4.0));
        assert!(approx(m(lin(std::f64::consts::PI)).unwrap(), 3.0));
        let aligned = DimKind::Aligned {
            ext1: P,
            ext2: Q,
            def_point: R,
            ext_line_rotation: 0.0,
        };
        assert!(approx(m(aligned).unwrap(), 5.0));
        let radius = DimKind::Radius {
            center: P,
            chord_point: Q,
            leader_length: 0.0,
        };
        assert!(approx(m(radius).unwrap(), 5.0));
        // Angular, 3 points: the sector holding the arc point.
        let ang = |def_point| DimKind::Angular3Pt {
            def_point,
            ext1: [1.0, 0.0, 0.0],
            ext2: [0.0, 2.0, 0.0],
            vertex: [0.0; 3],
        };
        let q = std::f64::consts::FRAC_PI_2;
        assert!(approx(m(ang([1.0, 1.0, 0.0])).unwrap(), q));
        assert!(approx(m(ang([-1.0, -1.0, 0.0])).unwrap(), 3.0 * q));
        assert_eq!(m(ang([0.0; 3])), None);
        // Angular, 2 lines crossing at (1, 1) at 45 degrees: each of the four
        // sectors.
        let two = |arc_point| DimKind::Angular2Line {
            arc_point,
            line1: [[0.0, 1.0, 0.0], [3.0, 1.0, 0.0]],
            line2: [[0.0, 0.0, 0.0], [2.0, 2.0, 0.0]],
        };
        let e = std::f64::consts::FRAC_PI_4;
        assert!(approx(m(two([3.0, 1.5])).unwrap(), e));
        assert!(approx(m(two([1.0, 3.0])).unwrap(), 3.0 * e));
        assert!(approx(m(two([-1.0, 0.5])).unwrap(), e));
        assert!(approx(m(two([1.0, -3.0])).unwrap(), 3.0 * e));
        // Parallel lines have no angle.
        let parallel = DimKind::Angular2Line {
            arc_point: [0.0, 5.0],
            line1: [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]],
            line2: [[0.0, 1.0, 0.0], [1.0, 1.0, 0.0]],
        };
        assert_eq!(m(parallel), None);
        // Large radial: center to chord point, as a radius.
        let large = DimKind::LargeRadial {
            center: P,
            chord_point: Q,
            unknown_40: 0.0,
            override_center: R,
            jog_point: [3.0; 3],
        };
        assert!(approx(m(large).unwrap(), 5.0));
        // Arc length: a quarter arc of radius 2 is pi long; the other way
        // round (arc point on the far side) three quarters.
        let arc = |arc_point| DimKind::ArcLength {
            arc_point,
            ext1: [2.0, 0.0, 0.0],
            ext2: [0.0, 2.0, 0.0],
            center: [0.0; 3],
            is_partial: false,
            start_angle: 0.0,
            end_angle: q,
            has_leader: false,
            leader1: [0.0; 3],
            leader2: [0.0; 3],
        };
        let pi = std::f64::consts::PI;
        assert!(approx(m(arc([3.0, 3.0, 0.0])).unwrap(), pi));
        assert!(approx(m(arc([-3.0, -3.0, 0.0])).unwrap(), 3.0 * pi));
        // A zero radius (the first extension point at the center) has no
        // length.
        let mut zero = arc([3.0, 3.0, 0.0]);
        if let DimKind::ArcLength { ext1, .. } = &mut zero {
            *ext1 = [0.0; 3];
        }
        assert_eq!(m(zero), None);
    }

    /// An ordinate measures from its definition point (the UCS origin) to
    /// the feature along the UCS X axis (flags 2 bit 0) or Y axis. DXF 51 is
    /// the negative of the angle from the OCS X axis to the UCS X axis.
    #[test]
    fn ordinate_measures_along_the_ucs_axes() {
        let ord = |flags2| DimKind::Ordinate {
            def_point: [1.0, 2.0, 0.0],
            feature_location: [4.0, 6.0, 0.0],
            leader_endpoint: [9.0, 9.0, 0.0],
            flags2,
        };
        let z = [0.0, 0.0, 1.0];
        let m = |k: DimKind, hd| k.computed_measurement(z, hd).unwrap();
        // The UCS is the OCS: X and Y differences.
        assert!(approx(m(ord(ORDINATE_X), 0.0), 3.0));
        assert!(approx(m(ord(0), 0.0), 4.0));
        // A UCS turned 90 degrees counterclockwise (DXF 51 = -90 degrees):
        // its X axis is the OCS Y axis and its Y axis the OCS -X axis.
        let hd = -std::f64::consts::FRAC_PI_2;
        assert!(approx(m(ord(ORDINATE_X), hd), 4.0));
        assert!(approx(m(ord(0), hd), 3.0));
        // A UCS turned 45 degrees: the X axis along (1, 1) / sqrt 2.
        let hd = -std::f64::consts::FRAC_PI_4;
        assert!(approx(m(ord(ORDINATE_X), hd), 7.0 / 2f64.sqrt()));
        assert!(approx(m(ord(0), hd), 1.0 / 2f64.sqrt()));
    }

    /// The measurement is taken in the dimension's OCS (arbitrary axis
    /// algorithm), and only for points in one plane parallel to it.
    #[test]
    fn measurement_uses_the_ocs() {
        // Extrusion +X: the OCS X axis is WCS Y and its Y axis WCS Z.
        let x = [1.0, 0.0, 0.0];
        let lin = |ext2, rotation| DimKind::Linear {
            ext1: [5.0, 0.0, 0.0],
            ext2,
            def_point: [0.0; 3],
            ext_line_rotation: 0.0,
            rotation,
        };
        let m = |k: DimKind| k.computed_measurement(x, 0.0);
        assert!(approx(m(lin([5.0, 3.0, 4.0], 0.0)).unwrap(), 3.0));
        let half_pi = std::f64::consts::FRAC_PI_2;
        assert!(approx(m(lin([5.0, 3.0, 4.0], half_pi)).unwrap(), 4.0));
        // The second point off the first's plane: no measurement.
        assert_eq!(m(lin([6.0, 3.0, 4.0], 0.0)), None);
        // Extrusion -Z: the OCS X axis is WCS -X.
        assert_eq!(
            to_ocs([1.0, 2.0, 3.0], [0.0, 0.0, -1.0]),
            Some([-1.0, 2.0, -3.0])
        );
        assert_eq!(to_ocs(P, [0.0; 3]), None);
    }

    /// The measurement is a copy derived from the geometry: written as
    /// stored while the geometry is unchanged since read, recomputed after
    /// an edit, and kept where the geometry is degenerate or not planar.
    #[test]
    fn measurement_follows_geometry_edits() {
        let d = dimension(DimKind::Aligned {
            ext1: P,
            ext2: Q,
            def_point: R,
            ext_line_rotation: 0.0,
        });
        // Built by hand: computed (the stored field holds 3).
        assert!(approx(d.measurement(), 5.0));
        // As read: the stored value, even where it differs from the
        // formula.
        let mut read = reread(&d);
        read.common.measurement = 4.75;
        let mut read = reread(&read);
        assert_eq!(read.common.measurement, 4.75);
        assert_eq!(read.measurement(), 4.75);
        // Edited: computed.
        if let DimKind::Aligned { ext2, .. } = &mut read.kind {
            *ext2 = [1.0, 12.0, 0.0];
        }
        assert!(approx(read.measurement(), 10.0));
        // Changing the extrusion is an edit too.
        if let DimKind::Aligned { ext2, .. } = &mut read.kind {
            *ext2 = Q;
        }
        assert_eq!(read.measurement(), 4.75);
        read.common.extrusion = [0.0, 0.0, -1.0];
        assert!(approx(read.measurement(), 5.0));
        // Non-text edits keep the stored value.
        read.common.extrusion = [0.0, 0.0, 1.0];
        read.common.user_text = "x".into();
        read.common.text_midpoint = [100.0, 100.0];
        assert_eq!(read.measurement(), 4.75);
        // An ordinate's horizontal direction is part of its geometry
        // (other kinds ignore it).
        let mut o = dimension(kinds()[0].clone());
        o.common.horiz_dir = 0.0;
        let mut o = reread(&o);
        o.common.measurement = 2.5;
        let mut o = reread(&o);
        assert_eq!(o.measurement(), 2.5);
        o.common.horiz_dir = -std::f64::consts::FRAC_PI_2;
        assert!(approx(o.measurement(), 4.0));
        o.common.horiz_dir = 0.0;
        assert_eq!(o.measurement(), 2.5);
        if let DimKind::Ordinate {
            feature_location, ..
        } = &mut o.kind
        {
            *feature_location = [50.0, 50.0, 0.0];
        }
        assert!(approx(o.measurement(), 49.0));
        read.common.horiz_dir = 1.0;
        assert_eq!(read.measurement(), 4.75);
        // Degenerate geometry keeps the stored value: an arc length whose
        // first point is its center.
        let mut a = reread(&dimension(kinds()[7].clone()));
        a.common.measurement = 1.25;
        let mut a = reread(&a);
        if let DimKind::ArcLength { ext1, center, .. } = &mut a.kind {
            *ext1 = *center;
        }
        assert_eq!(a.measurement(), 1.25);
        // And so does non-planar geometry.
        if let DimKind::Aligned { ext2, .. } = &mut read.kind {
            *ext2 = [4.0, 6.0, 1.0];
        }
        assert_eq!(read.measurement(), 4.75);
    }

    fn leader() -> Leader {
        Leader {
            unknown_bit: false,
            annot_type: 1,
            path_type: 1,
            points: vec![P, Q, R],
            origin: P,
            extrusion: [0.0, 0.0, 1.0],
            x_direction: [1.0, 0.0, 0.0],
            block_offset: [0.5; 3],
            endpoint_proj: [0.25; 3],
            box_size: None,
            hook_line_on_x_dir: true,
            arrowhead_on: true,
            unknown_bs: 3,
            unknown_b: [true, false],
            annotation: Handle(0x513),
            dimstyle: Handle(0x27),
        }
    }

    #[test]
    fn roundtrip_leader_and_tolerance() {
        roundtrip(&leader(), Leader::encode, Leader::decode, type_code::LEADER);
        let empty = Leader {
            points: vec![],
            annotation: Handle(0),
            ..leader()
        };
        roundtrip(&empty, Leader::encode, Leader::decode, type_code::LEADER);
        let t = Tolerance {
            insertion: P,
            x_direction: [1.0, 0.0, 0.0],
            extrusion: [0.0, 0.0, 1.0],
            text: "{\\Fgdt;p}%%v\n日本".into(),
            dimstyle: Handle(0x27),
        };
        roundtrip(
            &t,
            Tolerance::encode,
            Tolerance::decode,
            type_code::TOLERANCE,
        );
    }

    /// An R2004 leader stores its text box size; R2018 has no place for it.
    #[test]
    fn r2004_leader_box_size() {
        let mut d = BitWriter::new();
        d.write_b(false);
        d.write_bs(0);
        d.write_bs(0);
        d.write_bl(2);
        d.write_3bd(P);
        d.write_3bd(Q);
        for p in [P, [0.0, 0.0, 1.0], [1.0, 0.0, 0.0], [0.0; 3], [0.0; 3]] {
            d.write_3bd(p);
        }
        d.write_bd(1.5);
        d.write_bd(16.0);
        d.write_b(false);
        d.write_b(true);
        d.write_bs(0);
        d.write_b(false);
        d.write_b(false);
        let mut h = BitWriter::new();
        for value in [0x513, 0x27] {
            h.write_handle_ref(HandleRef { code: 5, value }).unwrap();
        }
        let bytes = assemble(Version::R2004, type_code::LEADER, &d, None, &h);
        let (mut r, _) = ObjReader::split(&bytes, Version::R2004, 30).unwrap();
        let l = Leader::decode(&mut r).unwrap();
        assert_eq!(r.data.remaining_bits(), 0);
        assert_eq!(l.box_size, Some([1.5, 16.0]));
        assert_eq!(l.points, [P, Q]);
        assert_eq!((l.annotation, l.dimstyle), (Handle(0x513), Handle(0x27)));
        let mut w = ObjWriter::new(Handle(0x80));
        l.encode(&mut w).unwrap();
        let out = w.finish(type_code::LEADER).unwrap();
        let back = Leader::decode(&mut reader(&out)).unwrap();
        assert_eq!(
            back,
            Leader {
                box_size: None,
                ..l
            }
        );
    }

    /// Entity common data for whole-object tests.
    pub(crate) fn entity_common() -> crate::object::EntityCommon {
        use crate::object::{EntityCommon, LinetypeRef, MaterialRef, ObjectCommon, PlotStyleRef};
        EntityCommon {
            obj: ObjectCommon {
                handle: Handle(0x80),
                owner: Handle(0),
                owner_relative: false,
                reactors: vec![],
                xdict: None,
                xdata: vec![],
                has_ds_binary_data: false,
            },
            entmode: 2,
            layer: Handle(0x10),
            linetype: LinetypeRef::ByLayer,
            plotstyle: PlotStyleRef::ByLayer,
            material: MaterialRef::ByLayer,
            color: crate::bits::Color::ByLayer,
            read_color: Default::default(),
            transparency: None,
            color_book: None,
            ltype_scale: 1.0,
            lineweight: 29,
            invisible: false,
            shadow_flags: 0,
            full_visual_style: None,
            face_visual_style: None,
            edge_visual_style: None,
            proxy_graphics: None,
            prev_next: None,
        }
    }

    /// Counts read from a file are checked against the stream left before
    /// anything is allocated for them; negative counts are refused.
    #[test]
    fn leader_counts_are_bounded() {
        let leader_with = |n: i32| {
            let mut d = BitWriter::new();
            d.write_b(false);
            d.write_bs(0);
            d.write_bs(0);
            d.write_bl(n);
            let none = BitWriter::new();
            assemble(Version::R2018, type_code::LEADER, &d, None, &none)
        };
        let bytes = leader_with(i32::MAX);
        assert!(matches!(
            Leader::decode(&mut reader(&bytes)),
            Err(Error::Truncated { .. })
        ));
        let bytes = leader_with(-1);
        assert!(matches!(
            Leader::decode(&mut reader(&bytes)),
            Err(Error::Corrupt(m)) if m.contains("negative")
        ));
    }

    /// ARC_DIMENSION and LARGE_RADIAL_DIMENSION are dispatched by their
    /// class's DXF name, whatever its number, and written with the number
    /// the target drawing gives that name.
    #[test]
    fn class_dimensions_dispatch_by_dxf_name() {
        use crate::object::{DecodeCtx, Entity, Object, decode_object, encode_object};
        use crate::section::classes::{DxfClass, ITEM_CLASS_ENTITY};
        let class = |n, name: &str| DxfClass {
            class_number: n,
            proxy_flags: 0x401,
            app_name: "ObjectDBX Classes".into(),
            cpp_class_name: "AcDbDim".into(),
            dxf_name: name.into(),
            was_zombie: false,
            item_class_id: ITEM_CLASS_ENTITY,
            instance_count: 0,
            dwg_version: 0,
            maintenance_version: 0,
            unknown: [0; 2],
        };
        let common = entity_common();
        let read = [
            class(500, "ACME"),
            class(501, "ARC_DIMENSION"),
            class(502, "LARGE_RADIAL_DIMENSION"),
        ];
        let other = [
            class(510, "LARGE_RADIAL_DIMENSION"),
            class(511, "ARC_DIMENSION"),
        ];
        for (i, name, number) in [
            (7, "ARC_DIMENSION", 511),
            (8, "LARGE_RADIAL_DIMENSION", 510),
        ] {
            let obj = Object::Entity {
                common: common.clone(),
                entity: Entity::Dimension(Box::new(measured(kinds()[i].clone()))),
            };
            let bytes = encode_object(&obj, &read).unwrap();
            let mut ctx = DecodeCtx::new(Version::R2018, 30, &read);
            let back = decode_object(&bytes, Handle(0x80), &mut ctx).unwrap();
            assert!(ctx.diagnostics.is_empty(), "{:?}", ctx.diagnostics);
            assert_eq!(back, obj);
            assert_eq!(back.type_name(), name);
            let out = encode_object(&back, &other).unwrap();
            let (_, pre) = ObjReader::split(&out, Version::R2018, 30).unwrap();
            assert_eq!(pre.type_code, number);
            assert!(matches!(encode_object(&back, &[]), Err(Error::Encode(m)) if m.contains(name)));
        }
    }
}
