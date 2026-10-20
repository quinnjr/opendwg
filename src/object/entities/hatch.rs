//! Hatches (ODS 20.4.75 HATCH).
//!
//! A hatch is bounded by paths, each a polyline or a list of edges (lines,
//! circular and elliptic arcs, splines), with the handles of the objects the
//! path was made from when the hatch is associative. A pattern hatch (not a
//! solid fill) stores its pattern definition lines, and R2004+ hatches a
//! gradient.
//!
//! The pattern definition lines are a cached expansion of the named pattern
//! (from `acad.pat` or a custom `.pat` file) scaled by
//! [`HatchPattern::scale`] and rotated by [`HatchPattern::angle`]. They are
//! kept as read: the pattern files are not available here, so editing the
//! scale or angle leaves the lines as they were (stale) until AutoCAD
//! regenerates the hatch. A user-defined pattern (type 0) is the exception:
//! its lines follow from the angle, spacing and double flag alone, so they
//! are generated for a hatch built in code and regenerated when those change
//! (see [`Hatch::written_pattern_lines`]).
//! The pixel size is a display cache, kept as read.
//!
//! The values the format stores as flags beside the data they describe are
//! derived from the data when written, so they cannot go stale: a path's
//! polyline bit (2) from its kind, the bulges-present bit and a spline
//! edge's rational bit from the values present (see [`PolylinePath`] and
//! [`HatchSpline`]), and a hatch's solid-fill bit from its pattern.
//!
//! Where the corpus or ACadSharp (MIT) differ from ODS, pinned by tests in
//! `tests/hatch_misc.rs`: a gradient color is a value and a `CMC` (ODS: a
//! BD, an unknown BS, a BL RGB and an ignored RC; the corpus stores BS 0
//! and CMC true colors), as ACadSharp `readHatch` reads it.
//!
//! Unconfirmed by the corpus, which has line edges and polyline paths only:
//! circular, elliptic and spline edges. They follow ODS; for R2010+ spline
//! fit data ODS stores the start and end tangents always, ACadSharp only
//! after a non-zero fit point count, and ODS is followed.

use crate::error::encode_error as encode_err;
use std::borrow::Cow;

use super::super::common::AsRead;
use super::super::stream::{ObjReader, ObjWriter};
use super::super::tables::{SOFT_POINTER, put};
use super::dim::{
    MIN_BD, MIN_BL, MIN_BS, check_fits, check_handles, corrupt, count, read_count, write_count,
};
use crate::bits::Color;
use crate::error::Result;
use crate::handle::Handle;
use crate::version::Version;

/// A raw double is 64 bits; a 2RD 128.
const RD: u64 = 64;
const MIN_2RD: u64 = 2 * RD;
/// The smallest gradient color: BD and a CMC (BS, BL, RC).
const MIN_GRADIENT_COLOR: u64 = MIN_BD + MIN_BS + MIN_BL + 8;
/// The smallest path: its flags, segment count and handle count (BL each).
const MIN_PATH: u64 = 3 * MIN_BL;
/// The smallest edge: its type byte and a spline's BL, B, B, BL, BL.
const MIN_EDGE: u64 = 8 + 3 * MIN_BL + 2;
/// The smallest pattern line: BD, 2BD, 2BD and a BS.
const MIN_PATTERN_LINE: u64 = 5 * MIN_BD + MIN_BS;

/// A path flag (DXF 92): the path is a polyline.
pub const PATH_POLYLINE: i32 = 2;
/// A path flag (DXF 92): the path is derived (a pixel size is stored).
pub const PATH_DERIVED: i32 = 4;

/// A hatch's gradient (R2004+; ODS 20.4.75). Hatches read from R2000 take
/// [`HatchGradient::default`]: not a gradient, no colors. (AutoCAD gives a
/// non-gradient hatch it saves as R2004 its leftover gradient settings, two
/// colors named `LINEAR` with a tint that varies, which are not derivable.)
#[derive(Debug, Clone, PartialEq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct HatchGradient {
    /// Non-zero for a gradient fill (DXF 450).
    pub enabled: i32,
    /// DXF 451.
    pub reserved: i32,
    /// DXF 460, 461.
    pub angle: f64,
    pub shift: f64,
    /// Non-zero for a single-color gradient (DXF 452).
    pub single_color: i32,
    /// DXF 462.
    pub tint: f64,
    /// DXF 453 count; each a value and color (DXF 463, 63, 421).
    pub colors: Vec<GradientColor>,
    /// DXF 470, e.g. `LINEAR`.
    pub name: String,
}

/// One color of a gradient. ODS lists a BD, an "unknown" BS, a BL RGB and
/// an "ignored" RC; the BS, BL and RC are a `CMC` (the corpus stores BS 0
/// and true colors, e.g. 0xC20000FF for DXF 63 = 5 and 421 = 255), as
/// ACadSharp (MIT) `readHatch` reads it.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct GradientColor {
    /// DXF 463.
    pub value: f64,
    /// DXF 63, 421.
    pub color: Color,
}

/// A hatch boundary path (DXF 92): a polyline or edges, and the objects it
/// was made from.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct BoundaryPath {
    /// DXF 92. The polyline bit ([`PATH_POLYLINE`]) is written from
    /// `edges`; [`PATH_DERIVED`] decides whether [`Hatch::pixel_size`] is
    /// stored.
    pub flags: i32,
    pub edges: PathEdges,
    /// The objects the path was made from (soft pointers, DXF 330).
    pub boundary_objects: Vec<Handle>,
}

/// What a boundary path is made of.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub enum PathEdges {
    Polyline(PolylinePath),
    Edges(Vec<HatchEdge>),
}

/// A polyline boundary path.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct PolylinePath {
    /// Whether bulges are stored (DXF 72). Written as set, or set when any
    /// bulge is non-zero, so an edited bulge is never dropped.
    pub has_bulges: bool,
    /// DXF 73.
    pub closed: bool,
    /// Points (DXF 10) and bulges (DXF 42, 0 when not stored).
    pub vertices: Vec<HatchVertex>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct HatchVertex {
    pub point: [f64; 2],
    pub bulge: f64,
}

/// A boundary path edge (DXF 72 type 1-4). Points are in the OCS.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub enum HatchEdge {
    Line {
        start: [f64; 2],
        end: [f64; 2],
    },
    CircularArc {
        center: [f64; 2],
        radius: f64,
        start_angle: f64,
        end_angle: f64,
        ccw: bool,
    },
    EllipticArc {
        center: [f64; 2],
        /// The end of the major axis, relative to the center.
        major_end: [f64; 2],
        /// Minor to major axis ratio.
        ratio: f64,
        start_angle: f64,
        end_angle: f64,
        ccw: bool,
    },
    Spline(Box<HatchSpline>),
}

/// A spline edge.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct HatchSpline {
    /// DXF 94.
    pub degree: i32,
    /// Whether weights are stored (DXF 73). Written as set, or set when any
    /// weight is not 1, so an edited weight is never dropped.
    pub rational: bool,
    /// DXF 74.
    pub periodic: bool,
    /// DXF 40.
    pub knots: Vec<f64>,
    /// Control points (DXF 10) and weights (DXF 42, 1 when not stored).
    pub control_points: Vec<([f64; 2], f64)>,
    /// R2010+ fit points (DXF 11); empty before.
    pub fit_points: Vec<[f64; 2]>,
    /// R2010+ start and end tangents (DXF 12, 13); zero before.
    pub start_tangent: [f64; 2],
    pub end_tangent: [f64; 2],
}

/// A pattern definition line (DXF 53, 43/44, 45/46, 79, 49).
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct PatternLine {
    pub angle: f64,
    pub base: [f64; 2],
    pub offset: [f64; 2],
    pub dashes: Vec<f64>,
}

/// The pattern of a hatch that is not a solid fill.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct HatchPattern {
    /// DXF 52.
    pub angle: f64,
    /// Scale or spacing (DXF 41).
    pub scale: f64,
    /// DXF 77.
    pub double: bool,
    /// The definition lines: a cached expansion of the named pattern at
    /// `angle` and `scale`, kept as read; for a user-defined pattern
    /// generated when the hatch was built in code or its settings changed
    /// (see [`Hatch::written_pattern_lines`]).
    pub lines: Vec<PatternLine>,
}

impl HatchPattern {
    /// The lines of a user-defined pattern (type 0): one family at `angle`
    /// through the origin, `scale` (the spacing) apart, and with `double` a
    /// second family at `angle` + 90 degrees. A line's offset is the
    /// spacing vector (0, spacing) rotated by the line's angle, as in the
    /// corpus's predefined patterns (ANSI31: 45 degrees, offset (-0.0884,
    /// 0.0884) for spacing 0.125).
    pub fn user_defined_lines(angle: f64, spacing: f64, double: bool) -> Vec<PatternLine> {
        let line = |a: f64| PatternLine {
            angle: a,
            base: [0.0, 0.0],
            offset: [-spacing * a.sin(), spacing * a.cos()],
            dashes: Vec::new(),
        };
        let mut lines = vec![line(angle)];
        if double {
            lines.push(line(angle + std::f64::consts::FRAC_PI_2));
        }
        lines
    }

    /// The settings the lines follow from.
    fn settings(&self, pattern_type: i16) -> PatternSettings {
        PatternSettings {
            pattern_type,
            angle_bits: self.angle.to_bits(),
            scale_bits: self.scale.to_bits(),
            double: self.double,
        }
    }
}

/// The settings a hatch's pattern lines follow from, as read from a file
/// (see [`Hatch::read_pattern`]): the pattern type, the angle and scale
/// (compared bit for bit) and the double flag. Opaque: it only records what
/// was read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct PatternSettings {
    pattern_type: i16,
    angle_bits: u64,
    scale_bits: u64,
    double: bool,
}

/// A hatch (ODS 20.4.75).
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct Hatch {
    /// R2004+; the default before.
    pub gradient: HatchGradient,
    /// The OCS Z of the hatch plane (DXF 30).
    pub elevation: f64,
    pub extrusion: [f64; 3],
    /// The pattern name (DXF 2), e.g. `ANSI31` or `SOLID`.
    pub pattern_name: String,
    /// DXF 71.
    pub associative: bool,
    pub paths: Vec<BoundaryPath>,
    /// 0 odd parity, 1 outermost, 2 whole area (DXF 75).
    pub style: i16,
    /// 0 user-defined, 1 predefined, 2 custom (DXF 76).
    pub pattern_type: i16,
    /// `None` for a solid fill (DXF 70 = 1).
    pub pattern: Option<HatchPattern>,
    /// DXF 47, stored when a path is derived ([`PATH_DERIVED`]); 0
    /// otherwise. A display cache, kept as read.
    pub pixel_size: f64,
    /// DXF 10.
    pub seed_points: Vec<[f64; 2]>,
    /// The pattern settings as read, for [`Hatch::written_pattern_lines`].
    #[cfg_attr(feature = "serde", serde(skip))]
    pub read_pattern: AsRead<PatternSettings>,
}

fn read_2rd_list(r: &mut ObjReader, what: &'static str) -> Result<Vec<[f64; 2]>> {
    let n = read_count(r, MIN_2RD, what)?;
    (0..n).map(|_| r.data.read_2rd()).collect()
}

fn write_2rd_list(w: &mut ObjWriter, points: &[[f64; 2]], what: &str) -> Result<()> {
    write_count(w, points.len(), what)?;
    for &p in points {
        w.data.write_2rd(p);
    }
    Ok(())
}

impl HatchGradient {
    fn decode(r: &mut ObjReader) -> Result<Self> {
        let d = &mut r.data;
        let enabled = d.read_bl()?;
        let reserved = d.read_bl()?;
        let angle = d.read_bd()?;
        let shift = d.read_bd()?;
        let single_color = d.read_bl()?;
        let tint = d.read_bd()?;
        let n = read_count(r, MIN_GRADIENT_COLOR, "gradient colors")?;
        let colors = (0..n)
            .map(|_| {
                Ok(GradientColor {
                    value: r.data.read_bd()?,
                    color: r.read_cmc()?,
                })
            })
            .collect::<Result<_>>()?;
        Ok(HatchGradient {
            enabled,
            reserved,
            angle,
            shift,
            single_color,
            tint,
            colors,
            name: r.text()?,
        })
    }

    fn encode(&self, w: &mut ObjWriter) -> Result<()> {
        let d = &mut w.data;
        d.write_bl(self.enabled);
        d.write_bl(self.reserved);
        d.write_bd(self.angle);
        d.write_bd(self.shift);
        d.write_bl(self.single_color);
        d.write_bd(self.tint);
        write_count(w, self.colors.len(), "gradient colors")?;
        for c in &self.colors {
            w.data.write_bd(c.value);
            w.write_cmc(&c.color)?;
        }
        w.text(&self.name)
    }
}

impl HatchEdge {
    fn decode(r: &mut ObjReader) -> Result<Self> {
        let d = &mut r.data;
        Ok(match d.read_rc()? {
            1 => HatchEdge::Line {
                start: d.read_2rd()?,
                end: d.read_2rd()?,
            },
            2 => HatchEdge::CircularArc {
                center: d.read_2rd()?,
                radius: d.read_bd()?,
                start_angle: d.read_bd()?,
                end_angle: d.read_bd()?,
                ccw: d.read_b()?,
            },
            3 => HatchEdge::EllipticArc {
                center: d.read_2rd()?,
                major_end: d.read_2rd()?,
                ratio: d.read_bd()?,
                start_angle: d.read_bd()?,
                end_angle: d.read_bd()?,
                ccw: d.read_b()?,
            },
            4 => HatchEdge::Spline(Box::new(HatchSpline::decode(r)?)),
            t => return Err(corrupt(format!("unknown hatch edge type {t}"))),
        })
    }

    fn encode(&self, w: &mut ObjWriter) -> Result<()> {
        let d = &mut w.data;
        match self {
            HatchEdge::Line { start, end } => {
                d.write_rc(1);
                d.write_2rd(*start);
                d.write_2rd(*end);
            }
            HatchEdge::CircularArc {
                center,
                radius,
                start_angle,
                end_angle,
                ccw,
            } => {
                d.write_rc(2);
                d.write_2rd(*center);
                d.write_bd(*radius);
                d.write_bd(*start_angle);
                d.write_bd(*end_angle);
                d.write_b(*ccw);
            }
            HatchEdge::EllipticArc {
                center,
                major_end,
                ratio,
                start_angle,
                end_angle,
                ccw,
            } => {
                d.write_rc(3);
                d.write_2rd(*center);
                d.write_2rd(*major_end);
                d.write_bd(*ratio);
                d.write_bd(*start_angle);
                d.write_bd(*end_angle);
                d.write_b(*ccw);
            }
            HatchEdge::Spline(s) => {
                d.write_rc(4);
                s.encode(w)?;
            }
        }
        Ok(())
    }
}

impl HatchSpline {
    fn decode(r: &mut ObjReader) -> Result<Self> {
        let d = &mut r.data;
        let degree = d.read_bl()?;
        let rational = d.read_b()?;
        let periodic = d.read_b()?;
        let n_knots = count(d.read_bl()?, "spline edge knots")?;
        let n_control = count(r.data.read_bl()?, "spline edge control points")?;
        check_fits(r, n_knots, MIN_BD, "spline edge knots")?;
        let knots = (0..n_knots)
            .map(|_| r.data.read_bd())
            .collect::<Result<_>>()?;
        check_fits(r, n_control, MIN_2RD, "spline edge control points")?;
        let control_points = (0..n_control)
            .map(|_| {
                let p = r.data.read_2rd()?;
                let weight = if rational { r.data.read_bd()? } else { 1.0 };
                Ok((p, weight))
            })
            .collect::<Result<_>>()?;
        let (fit_points, start_tangent, end_tangent) = if r.version >= Version::R2010 {
            let fit = read_2rd_list(r, "spline edge fit points")?;
            (fit, r.data.read_2rd()?, r.data.read_2rd()?)
        } else {
            (Vec::new(), [0.0; 2], [0.0; 2])
        };
        Ok(HatchSpline {
            degree,
            rational,
            periodic,
            knots,
            control_points,
            fit_points,
            start_tangent,
            end_tangent,
        })
    }

    /// Whether weights are written: when flagged or when any weight is not 1.
    fn writes_weights(&self) -> bool {
        self.rational || self.control_points.iter().any(|&(_, w)| w != 1.0)
    }

    fn encode(&self, w: &mut ObjWriter) -> Result<()> {
        let rational = self.writes_weights();
        w.data.write_bl(self.degree);
        w.data.write_b(rational);
        w.data.write_b(self.periodic);
        write_count(w, self.knots.len(), "spline edge knots")?;
        write_count(w, self.control_points.len(), "spline edge control points")?;
        for &k in &self.knots {
            w.data.write_bd(k);
        }
        for &(p, weight) in &self.control_points {
            w.data.write_2rd(p);
            if rational {
                w.data.write_bd(weight);
            }
        }
        write_2rd_list(w, &self.fit_points, "spline edge fit points")?;
        w.data.write_2rd(self.start_tangent);
        w.data.write_2rd(self.end_tangent);
        Ok(())
    }
}

impl PolylinePath {
    fn decode(r: &mut ObjReader) -> Result<Self> {
        let has_bulges = r.data.read_b()?;
        let closed = r.data.read_b()?;
        let n = read_count(r, MIN_2RD, "hatch polyline vertices")?;
        let vertices = (0..n)
            .map(|_| {
                Ok(HatchVertex {
                    point: r.data.read_2rd()?,
                    bulge: if has_bulges { r.data.read_bd()? } else { 0.0 },
                })
            })
            .collect::<Result<_>>()?;
        Ok(PolylinePath {
            has_bulges,
            closed,
            vertices,
        })
    }

    /// Whether bulges are written: when flagged or when any is non-zero.
    fn writes_bulges(&self) -> bool {
        self.has_bulges || self.vertices.iter().any(|v| v.bulge != 0.0)
    }

    fn encode(&self, w: &mut ObjWriter) -> Result<()> {
        let bulges = self.writes_bulges();
        w.data.write_b(bulges);
        w.data.write_b(self.closed);
        write_count(w, self.vertices.len(), "hatch polyline vertices")?;
        for v in &self.vertices {
            w.data.write_2rd(v.point);
            if bulges {
                w.data.write_bd(v.bulge);
            }
        }
        Ok(())
    }
}

impl BoundaryPath {
    fn decode(r: &mut ObjReader) -> Result<Self> {
        let flags = r.data.read_bl()?;
        let edges = if flags & PATH_POLYLINE != 0 {
            PathEdges::Polyline(PolylinePath::decode(r)?)
        } else {
            let n = read_count(r, MIN_EDGE, "hatch path edges")?;
            PathEdges::Edges(
                (0..n)
                    .map(|_| HatchEdge::decode(r))
                    .collect::<Result<_>>()?,
            )
        };
        let n = count(r.data.read_bl()?, "hatch boundary objects")?;
        check_handles(r, n, "hatch boundary objects")?;
        let boundary_objects = (0..n).map(|_| r.href_abs()).collect::<Result<_>>()?;
        Ok(BoundaryPath {
            flags,
            edges,
            boundary_objects,
        })
    }

    fn encode(&self, w: &mut ObjWriter) -> Result<()> {
        let polyline = matches!(self.edges, PathEdges::Polyline(_));
        let flags = if polyline {
            self.flags | PATH_POLYLINE
        } else {
            self.flags & !PATH_POLYLINE
        };
        w.data.write_bl(flags);
        match &self.edges {
            PathEdges::Polyline(p) => p.encode(w)?,
            PathEdges::Edges(edges) => {
                write_count(w, edges.len(), "hatch path edges")?;
                for e in edges {
                    e.encode(w)?;
                }
            }
        }
        write_count(w, self.boundary_objects.len(), "hatch boundary objects")?;
        for &h in &self.boundary_objects {
            put(w, SOFT_POINTER, h)?;
        }
        Ok(())
    }
}

impl PatternLine {
    fn decode(r: &mut ObjReader) -> Result<Self> {
        let d = &mut r.data;
        let angle = d.read_bd()?;
        let base = [d.read_bd()?, d.read_bd()?];
        let offset = [d.read_bd()?, d.read_bd()?];
        let n = count(i32::from(d.read_bs()?), "pattern line dashes")?;
        check_fits(r, n, MIN_BD, "pattern line dashes")?;
        let dashes = (0..n).map(|_| r.data.read_bd()).collect::<Result<_>>()?;
        Ok(PatternLine {
            angle,
            base,
            offset,
            dashes,
        })
    }

    fn encode(&self, w: &mut ObjWriter) -> Result<()> {
        let d = &mut w.data;
        d.write_bd(self.angle);
        self.base.into_iter().for_each(|v| d.write_bd(v));
        self.offset.into_iter().for_each(|v| d.write_bd(v));
        d.write_bs(short_count(self.dashes.len(), "pattern line dashes")?);
        for &x in &self.dashes {
            d.write_bd(x);
        }
        Ok(())
    }
}

/// A `BS` count.
fn short_count(n: usize, what: &str) -> Result<i16> {
    i16::try_from(n).map_err(|_| encode_err(format!("too many {what} to encode")))
}

impl HatchPattern {
    fn decode(r: &mut ObjReader) -> Result<Self> {
        let d = &mut r.data;
        let angle = d.read_bd()?;
        let scale = d.read_bd()?;
        let double = d.read_b()?;
        let n = count(i32::from(d.read_bs()?), "pattern lines")?;
        check_fits(r, n, MIN_PATTERN_LINE, "pattern lines")?;
        let lines = (0..n)
            .map(|_| PatternLine::decode(r))
            .collect::<Result<_>>()?;
        Ok(HatchPattern {
            angle,
            scale,
            double,
            lines,
        })
    }

    fn encode(&self, w: &mut ObjWriter, lines: &[PatternLine]) -> Result<()> {
        w.data.write_bd(self.angle);
        w.data.write_bd(self.scale);
        w.data.write_b(self.double);
        w.data.write_bs(short_count(lines.len(), "pattern lines")?);
        for l in lines {
            l.encode(w)?;
        }
        Ok(())
    }
}

impl Hatch {
    /// Whether the hatch is a solid fill (DXF 70): it has no pattern.
    pub fn is_solid(&self) -> bool {
        self.pattern.is_none()
    }

    /// The pattern lines written: [`HatchPattern::lines`], except for a
    /// user-defined pattern (type 0) not read from a file, or read from one
    /// but whose type, angle, spacing or double flag changed since: then its
    /// lines are generated ([`HatchPattern::user_defined_lines`]). Other
    /// patterns' lines are an expansion of a pattern file and are written as
    /// they are.
    pub fn written_pattern_lines(&self) -> Option<Cow<'_, [PatternLine]>> {
        let p = self.pattern.as_ref()?;
        let changed = match self.read_pattern.get() {
            Some(read) => *read != p.settings(self.pattern_type),
            // Built in code: no lines as read to keep.
            None => true,
        };
        Some(if self.pattern_type == 0 && changed {
            Cow::Owned(HatchPattern::user_defined_lines(p.angle, p.scale, p.double))
        } else {
            Cow::Borrowed(&p.lines)
        })
    }

    /// Whether the pixel size is stored: a path is derived.
    fn has_pixel_size(&self) -> bool {
        self.paths.iter().any(|p| p.flags & PATH_DERIVED != 0)
    }

    pub(crate) fn decode(r: &mut ObjReader) -> Result<Self> {
        let gradient = if r.version >= Version::R2004 {
            HatchGradient::decode(r)?
        } else {
            HatchGradient::default()
        };
        let elevation = r.data.read_bd()?;
        let extrusion = r.data.read_3bd()?;
        let pattern_name = r.text()?;
        let solid = r.data.read_b()?;
        let associative = r.data.read_b()?;
        let n = read_count(r, MIN_PATH, "hatch paths")?;
        let paths = (0..n)
            .map(|_| BoundaryPath::decode(r))
            .collect::<Result<Vec<_>>>()?;
        let style = r.data.read_bs()?;
        let pattern_type = r.data.read_bs()?;
        let pattern = if solid {
            None
        } else {
            Some(HatchPattern::decode(r)?)
        };
        let mut hatch = Hatch {
            gradient,
            elevation,
            extrusion,
            pattern_name,
            associative,
            paths,
            style,
            pattern_type,
            pattern,
            pixel_size: 0.0,
            seed_points: Vec::new(),
            read_pattern: AsRead::default(),
        };
        if let Some(p) = &hatch.pattern {
            hatch.read_pattern = AsRead::new(p.settings(pattern_type));
        }
        if hatch.has_pixel_size() {
            hatch.pixel_size = r.data.read_bd()?;
        }
        hatch.seed_points = read_2rd_list(r, "hatch seed points")?;
        Ok(hatch)
    }

    pub(crate) fn encode(&self, w: &mut ObjWriter) -> Result<()> {
        self.gradient.encode(w)?;
        w.data.write_bd(self.elevation);
        w.data.write_3bd(self.extrusion);
        w.text(&self.pattern_name)?;
        w.data.write_b(self.is_solid());
        w.data.write_b(self.associative);
        write_count(w, self.paths.len(), "hatch paths")?;
        for p in &self.paths {
            p.encode(w)?;
        }
        w.data.write_bs(self.style);
        w.data.write_bs(self.pattern_type);
        if let (Some(p), Some(lines)) = (&self.pattern, self.written_pattern_lines()) {
            p.encode(w, &lines)?;
        }
        if self.has_pixel_size() {
            w.data.write_bd(self.pixel_size);
        }
        write_2rd_list(w, &self.seed_points, "hatch seed points")
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

    fn spline() -> HatchSpline {
        HatchSpline {
            degree: 3,
            rational: true,
            periodic: false,
            knots: vec![0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0],
            control_points: vec![
                ([0.0, 0.0], 1.0),
                ([1.0, 2.0], 0.5),
                ([2.0, 2.0], 0.5),
                ([3.0, 0.0], 1.0),
            ],
            fit_points: vec![[0.0, 0.0], [3.0, 0.0]],
            start_tangent: [1.0, 0.0],
            end_tangent: [0.0, -1.0],
        }
    }

    /// A hatch with every kind of path and edge, a pattern and a gradient.
    pub(crate) fn hatch() -> Hatch {
        Hatch {
            gradient: HatchGradient {
                enabled: 1,
                reserved: 0,
                angle: 0.5,
                shift: 0.25,
                single_color: 0,
                tint: 0.75,
                colors: vec![
                    GradientColor {
                        value: 0.0,
                        color: Color::Rgb { r: 0, g: 0, b: 255 },
                    },
                    GradientColor {
                        value: 0.5,
                        color: Color::Index(3),
                    },
                    GradientColor {
                        value: 1.0,
                        color: Color::Book {
                            rgb: 0xFF_FF00,
                            color_name: "Gelb".into(),
                            book_name: "RAL".into(),
                        },
                    },
                ],
                name: "LINEAR".into(),
            },
            elevation: 1.5,
            extrusion: [0.0, 0.0, 1.0],
            pattern_name: "ANSI31 ü".into(),
            associative: true,
            paths: vec![
                BoundaryPath {
                    flags: 1 | PATH_DERIVED,
                    edges: PathEdges::Edges(vec![
                        HatchEdge::Line {
                            start: [0.0, 0.0],
                            end: [3.0, 0.0],
                        },
                        HatchEdge::CircularArc {
                            center: [3.0, 1.0],
                            radius: 1.0,
                            start_angle: -1.5,
                            end_angle: 1.5,
                            ccw: true,
                        },
                        HatchEdge::EllipticArc {
                            center: [1.5, 2.0],
                            major_end: [1.5, 0.0],
                            ratio: 0.5,
                            start_angle: 0.0,
                            end_angle: 3.0,
                            ccw: false,
                        },
                        HatchEdge::Spline(Box::new(spline())),
                    ]),
                    boundary_objects: vec![Handle(0x90), Handle(0x91)],
                },
                BoundaryPath {
                    flags: PATH_POLYLINE | 16,
                    edges: PathEdges::Polyline(PolylinePath {
                        has_bulges: true,
                        closed: true,
                        vertices: vec![
                            HatchVertex {
                                point: [0.5, 0.5],
                                bulge: 0.0,
                            },
                            HatchVertex {
                                point: [1.0, 0.5],
                                bulge: 0.4,
                            },
                            HatchVertex {
                                point: [1.0, 1.0],
                                bulge: 0.0,
                            },
                        ],
                    }),
                    boundary_objects: vec![],
                },
            ],
            style: 1,
            pattern_type: 1,
            pattern: Some(HatchPattern {
                angle: 0.0,
                scale: 2.0,
                double: true,
                lines: vec![
                    PatternLine {
                        angle: std::f64::consts::FRAC_PI_4,
                        base: [0.0, 0.0],
                        offset: [-0.088, 0.088],
                        dashes: vec![],
                    },
                    PatternLine {
                        angle: 1.5,
                        base: [2.0, 0.0],
                        offset: [-12.0, 12.0],
                        dashes: vec![12.0, -12.0],
                    },
                ],
            }),
            pixel_size: 0.03,
            seed_points: vec![[1.0, 1.0], [2.0, 2.0]],
            read_pattern: AsRead::default(),
        }
    }

    #[test]
    fn roundtrip_hatch() {
        roundtrip(&hatch(), Hatch::encode, Hatch::decode, type_code::HATCH);
        // A solid fill, no derived path (no pixel size), no seed points.
        let mut solid = hatch();
        solid.pattern = None;
        solid.pixel_size = 0.0;
        solid.paths[0].flags = 1;
        solid.seed_points.clear();
        roundtrip(&solid, Hatch::encode, Hatch::decode, type_code::HATCH);
        assert!(solid.is_solid());
    }

    fn reread(h: &Hatch) -> Hatch {
        let mut w = ObjWriter::new(Handle(0x80));
        h.encode(&mut w).unwrap();
        let bytes = w.finish(type_code::HATCH).unwrap();
        let mut r = reader(&bytes);
        Hatch::decode(&mut r).unwrap()
    }

    /// The format's flags are written from the data: a path's polyline bit
    /// from its kind, bulges and weights when any is set, the solid bit
    /// from the pattern.
    #[test]
    fn stored_flags_follow_the_data() {
        let mut h = hatch();
        // An edge path flagged as a polyline, a polyline path not.
        h.paths[0].flags |= PATH_POLYLINE;
        h.paths[1].flags &= !PATH_POLYLINE;
        // A bulge and a weight set with their flags clear.
        let PathEdges::Polyline(p) = &mut h.paths[1].edges else {
            unreachable!()
        };
        p.has_bulges = false;
        let PathEdges::Edges(edges) = &mut h.paths[0].edges else {
            unreachable!()
        };
        let HatchEdge::Spline(s) = &mut edges[3] else {
            unreachable!()
        };
        s.rational = false;
        let back = reread(&h);
        assert_eq!(back.paths[0].flags, 1 | PATH_DERIVED);
        assert_eq!(back.paths[1].flags, PATH_POLYLINE | 16);
        assert_eq!(back, hatch());
        // Without set bulges or weights the flags stay clear.
        let mut h = hatch();
        let PathEdges::Polyline(p) = &mut h.paths[1].edges else {
            unreachable!()
        };
        p.has_bulges = false;
        p.vertices.iter_mut().for_each(|v| v.bulge = 0.0);
        let PathEdges::Edges(edges) = &mut h.paths[0].edges else {
            unreachable!()
        };
        let HatchEdge::Spline(s) = &mut edges[3] else {
            unreachable!()
        };
        s.rational = false;
        s.control_points.iter_mut().for_each(|c| c.1 = 1.0);
        assert_eq!(reread(&h), h);
        // A flagged polyline with zero bulges keeps its flag.
        let mut h = hatch();
        let PathEdges::Polyline(p) = &mut h.paths[1].edges else {
            unreachable!()
        };
        p.vertices.iter_mut().for_each(|v| v.bulge = 0.0);
        assert_eq!(reread(&h), h);
    }

    /// An R2000 hatch: no gradient (the default), and a pre-R2010 spline
    /// edge without fit data.
    #[test]
    fn r2000_hatch() {
        let mut d = BitWriter::new();
        d.write_bd(0.0);
        d.write_3bd([0.0, 0.0, 1.0]);
        d.write_tv_raw(b"ANSI31").unwrap();
        d.write_b(false); // pattern
        d.write_b(true);
        d.write_bl(1); // one path
        d.write_bl(0);
        d.write_bl(1); // one edge: a spline
        d.write_rc(4);
        d.write_bl(1);
        d.write_b(false);
        d.write_b(false);
        d.write_bl(2);
        d.write_bl(2);
        d.write_bd(0.0);
        d.write_bd(1.0);
        d.write_2rd([0.0, 0.0]);
        d.write_2rd([1.0, 1.0]);
        d.write_bl(1); // boundary objects
        d.write_bs(0);
        d.write_bs(1);
        d.write_bd(0.0);
        d.write_bd(1.0);
        d.write_b(false);
        d.write_bs(0); // no lines
        d.write_bl(0); // no seed points
        let mut h = BitWriter::new();
        h.write_handle_ref(HandleRef {
            code: 4,
            value: 0x55,
        })
        .unwrap();
        let bytes = assemble(Version::R2000, type_code::HATCH, &d, None, &h);
        let (mut r, _) = ObjReader::split(&bytes, Version::R2000, 30).unwrap();
        let x = Hatch::decode(&mut r).unwrap();
        assert_eq!(r.data.remaining_bits(), 0);
        assert_eq!(x.gradient, HatchGradient::default());
        assert_eq!(x.pattern_name, "ANSI31");
        assert_eq!(x.paths[0].boundary_objects, [Handle(0x55)]);
        let PathEdges::Edges(edges) = &x.paths[0].edges else {
            panic!("{:?}", x.paths[0].edges)
        };
        let HatchEdge::Spline(s) = &edges[0] else {
            panic!("{edges:?}")
        };
        assert!(s.fit_points.is_empty());
        assert_eq!(s.control_points[1], ([1.0, 1.0], 1.0));
        // As R2018 it reads back equal.
        assert_eq!(reread(&x), x);
    }

    /// A user-defined (type 0) pattern's lines follow its settings: written
    /// as read while they are unchanged, regenerated once the angle, spacing
    /// or double flag changes. Other pattern types keep their lines.
    #[test]
    fn user_defined_pattern_lines_follow_the_settings() {
        // A hand-made type 0 hatch as AutoCAD would store it: spacing 0.5 at
        // 30 degrees, double, with the cached lines.
        let a = 30f64.to_radians();
        let mut h = hatch();
        h.pattern_type = 0;
        let p = h.pattern.as_mut().unwrap();
        (p.angle, p.scale, p.double) = (a, 0.5, true);
        p.lines = HatchPattern::user_defined_lines(a, 0.5, true);
        let close = |x: f64, y: f64| (x - y).abs() < 1e-12;
        let l = &p.lines;
        assert_eq!(l.len(), 2);
        assert!(close(l[0].angle, a) && close(l[1].angle, a + std::f64::consts::FRAC_PI_2));
        assert!(close(l[0].offset[0], -0.25) && close(l[0].offset[1], 0.5 * a.cos()));
        assert!(close(l[1].offset[0], -0.5 * a.cos()) && close(l[1].offset[1], -0.25));
        assert!(
            l.iter()
                .all(|l| l.base == [0.0, 0.0] && l.dashes.is_empty())
        );
        // Read back, it remembers its settings.
        let read = reread(&h);
        assert!(read.read_pattern.get().is_some());
        // Unchanged: the lines as read (even odd ones).
        let mut odd = read.clone();
        odd.pattern.as_mut().unwrap().lines[0].dashes = vec![1.0];
        let odd_back = reread(&odd);
        assert_eq!(odd_back.pattern.unwrap().lines[0].dashes, [1.0]);
        // Spacing, angle or double changed: regenerated.
        for edit in [
            |p: &mut HatchPattern| p.scale = 2.0,
            |p: &mut HatchPattern| p.angle = 0.0,
            |p: &mut HatchPattern| p.double = false,
        ] {
            let mut e = read.clone();
            edit(e.pattern.as_mut().unwrap());
            let p = e.pattern.as_ref().unwrap();
            let want = HatchPattern::user_defined_lines(p.angle, p.scale, p.double);
            assert_eq!(reread(&e).pattern.unwrap().lines, want);
        }
        // A predefined pattern keeps its (now stale) lines.
        let mut pre = hatch();
        pre.pattern_type = 1;
        let pre = reread(&pre);
        let mut e = pre.clone();
        e.pattern.as_mut().unwrap().scale = 9.0;
        assert_eq!(
            reread(&e).pattern.unwrap().lines,
            pre.pattern.unwrap().lines
        );
        // Changed to user-defined: regenerated.
        let mut e = reread(&hatch());
        e.pattern_type = 0;
        let p = e.pattern.as_ref().unwrap();
        let want = HatchPattern::user_defined_lines(p.angle, p.scale, p.double);
        assert_eq!(reread(&e).pattern.unwrap().lines, want);
    }

    /// A user-defined hatch built in code (nothing read) gets its lines
    /// generated from its settings, whatever lines it holds.
    #[test]
    fn user_defined_pattern_built_in_code_gets_its_lines() {
        let mut h = hatch();
        h.pattern_type = 0;
        assert!(h.read_pattern.get().is_none());
        let p = h.pattern.as_mut().unwrap();
        (p.angle, p.scale, p.double) = (0.25, 0.75, true);
        for lines in [vec![], hatch().pattern.unwrap().lines] {
            h.pattern.as_mut().unwrap().lines = lines;
            let want = HatchPattern::user_defined_lines(0.25, 0.75, true);
            assert_eq!(h.written_pattern_lines().unwrap().as_ref(), want.as_slice());
            assert_eq!(reread(&h).pattern.unwrap().lines, want);
        }
        // A predefined one built in code keeps the lines it holds.
        let pre = hatch();
        assert_eq!(pre.pattern_type, 1);
        assert_eq!(
            reread(&pre).pattern.unwrap().lines,
            pre.pattern.unwrap().lines
        );
    }

    /// Counts beyond the data left fail before anything is allocated, as do
    /// negative counts and an unknown edge type.
    #[test]
    fn hatch_counts_are_bounded() {
        // The fields before the paths: an empty gradient, the elevation and
        // extrusion, the solid bit and the path count.
        let head = |solid: bool, paths: i32| {
            let mut d = BitWriter::new();
            d.write_bl(0);
            d.write_bl(0);
            d.write_bd(0.0);
            d.write_bd(0.0);
            d.write_bl(0);
            d.write_bd(0.0);
            d.write_bl(0); // no gradient colors
            d.write_bd(0.0);
            d.write_3bd([0.0, 0.0, 1.0]);
            d.write_b(solid);
            d.write_b(false);
            d.write_bl(paths);
            d
        };
        let decode = |d: &BitWriter| {
            let mut s = BitWriter::new();
            s.write_tu("").unwrap(); // gradient name
            s.write_tu("ANSI31").unwrap();
            let h = BitWriter::new();
            let bytes = assemble(Version::R2018, type_code::HATCH, d, Some(&s), &h);
            let mut r = reader(&bytes);
            Hatch::decode(&mut r)
        };
        for n in [i32::MAX, -1] {
            assert!(decode(&head(true, n)).is_err(), "{n}");
        }
        // One edge path with i32::MAX edges.
        let mut d = head(true, 1);
        d.write_bl(0);
        d.write_bl(i32::MAX);
        assert!(decode(&d).is_err());
        // An unknown edge type.
        let mut d = head(true, 1);
        d.write_bl(0);
        d.write_bl(1);
        d.write_rc(9);
        d.write_2rd([0.0; 2]); // room for an edge
        let e = decode(&d).unwrap_err();
        assert!(e.to_string().contains("edge type 9"), "{e}");
        // A boundary object count beyond the (empty) handle stream.
        let mut d = head(true, 1);
        d.write_bl(PATH_POLYLINE);
        d.write_b(false);
        d.write_b(true);
        d.write_bl(0);
        d.write_bl(1);
        assert!(decode(&d).is_err());
        // i32::MAX polyline vertices.
        let mut d = head(true, 1);
        d.write_bl(PATH_POLYLINE);
        d.write_b(false);
        d.write_b(true);
        d.write_bl(i32::MAX);
        assert!(decode(&d).is_err());
        // Spline edge knot and control point counts.
        for (knots, control) in [(i32::MAX, 0), (0, i32::MAX), (-1, 0)] {
            let mut d = head(true, 1);
            d.write_bl(0);
            d.write_bl(1);
            d.write_rc(4);
            d.write_bl(3);
            d.write_b(false);
            d.write_b(false);
            d.write_bl(knots);
            d.write_bl(control);
            assert!(decode(&d).is_err(), "{knots} {control}");
        }
        // Pattern line and dash counts: one line with `dashes` dashes.
        let pattern = |lines: i16, dashes: i16| {
            let mut d = head(false, 0);
            d.write_bs(0);
            d.write_bs(1);
            d.write_bd(0.0);
            d.write_bd(1.0);
            d.write_b(false);
            d.write_bs(lines);
            for _ in 0..5 {
                d.write_bd(0.0);
            }
            d.write_bs(dashes);
            d.write_bl(0); // no seed points
            d
        };
        assert!(decode(&pattern(1, 0)).is_ok());
        assert!(decode(&pattern(i16::MAX, 0)).is_err());
        assert!(decode(&pattern(-1, 0)).is_err());
        assert!(decode(&pattern(1, -1)).is_err());
        assert!(decode(&pattern(1, i16::MAX)).is_err());
        // i32::MAX seed points.
        let mut d = head(true, 0);
        d.write_bs(0);
        d.write_bs(1);
        d.write_bl(i32::MAX);
        assert!(decode(&d).is_err());
    }
}
