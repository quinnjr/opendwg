//! Basic entities (ODS 20.4.6-20.4.8, 20.4.18-20.4.21, 20.4.31, 20.4.32,
//! 20.4.35-20.4.37, 20.4.39, 20.4.42, 20.4.43): lines, points, circles, arcs,
//! ellipses, rays, xlines, solids, traces, 3D faces, shapes, and the BLOCK,
//! ENDBLK and SEQEND markers.
//!
//! Where R2000+ files omit values equal to a default (the "zero" bits of
//! LINE and 3DFACE, `BT` thickness, `BE` extrusion, `DD` coordinates), the
//! encoder chooses the shortest form, as AutoCAD does; the corpus test
//! `corpus_basic_entities_r2018_bytes` checks that this reproduces AutoCAD's bytes.

use super::super::stream::{ObjReader, ObjWriter};
use super::super::tables::{HARD_POINTER, put};
use crate::error::Result;
use crate::handle::Handle;

/// Whether `v` is +0.0 (a stored -0.0 is kept by writing it).
fn is_zero(v: f64) -> bool {
    v.to_bits() == 0
}

/// A line (ODS 20.4.21).
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Line {
    /// Start point.
    pub start: [f64; 3],
    /// End point.
    pub end: [f64; 3],
    /// Thickness (extrusion along the normal).
    pub thickness: f64,
    /// Normal (extrusion direction).
    pub extrusion: [f64; 3],
}

impl Line {
    /// A line from `start` to `end`, with no thickness, in the XY plane
    /// (extrusion +Z).
    pub fn new(start: [f64; 3], end: [f64; 3]) -> Line {
        Line {
            start,
            end,
            thickness: 0.0,
            extrusion: [0.0, 0.0, 1.0],
        }
    }

    pub(crate) fn decode(r: &mut ObjReader) -> Result<Self> {
        let d = &mut r.data;
        let zs_zero = d.read_b()?;
        let sx = d.read_rd()?;
        let ex = d.read_dd(sx)?;
        let sy = d.read_rd()?;
        let ey = d.read_dd(sy)?;
        let (sz, ez) = if zs_zero {
            (0.0, 0.0)
        } else {
            let sz = d.read_rd()?;
            (sz, d.read_dd(sz)?)
        };
        Ok(Line {
            start: [sx, sy, sz],
            end: [ex, ey, ez],
            thickness: d.read_bt()?,
            extrusion: d.read_be()?,
        })
    }

    pub(crate) fn encode(&self, w: &mut ObjWriter) -> Result<()> {
        let d = &mut w.data;
        let zs_zero = is_zero(self.start[2]) && is_zero(self.end[2]);
        d.write_b(zs_zero);
        d.write_rd(self.start[0]);
        d.write_dd(self.end[0], self.start[0]);
        d.write_rd(self.start[1]);
        d.write_dd(self.end[1], self.start[1]);
        if !zs_zero {
            d.write_rd(self.start[2]);
            d.write_dd(self.end[2], self.start[2]);
        }
        d.write_bt(self.thickness);
        d.write_be(self.extrusion);
        Ok(())
    }
}

/// A point (ODS 20.4.31).
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct Point {
    pub position: [f64; 3],
    pub thickness: f64,
    pub extrusion: [f64; 3],
    /// The angle of the UCS X axis when the point was drawn (DXF 50).
    pub x_axis_angle: f64,
}

impl Point {
    pub(crate) fn decode(r: &mut ObjReader) -> Result<Self> {
        let d = &mut r.data;
        Ok(Point {
            position: d.read_3bd()?,
            thickness: d.read_bt()?,
            extrusion: d.read_be()?,
            x_axis_angle: d.read_bd()?,
        })
    }

    pub(crate) fn encode(&self, w: &mut ObjWriter) -> Result<()> {
        let d = &mut w.data;
        d.write_3bd(self.position);
        d.write_bt(self.thickness);
        d.write_be(self.extrusion);
        d.write_bd(self.x_axis_angle);
        Ok(())
    }
}

/// A circle (ODS 20.4.20).
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Circle {
    /// Center (in the entity's coordinate system).
    pub center: [f64; 3],
    /// Radius.
    pub radius: f64,
    /// Thickness (extrusion along the normal).
    pub thickness: f64,
    /// Normal (extrusion direction).
    pub extrusion: [f64; 3],
}

impl Circle {
    /// A circle of `radius` around `center`, with no thickness, in the XY
    /// plane (extrusion +Z).
    pub fn new(center: [f64; 3], radius: f64) -> Circle {
        Circle {
            center,
            radius,
            thickness: 0.0,
            extrusion: [0.0, 0.0, 1.0],
        }
    }

    pub(crate) fn decode(r: &mut ObjReader) -> Result<Self> {
        let d = &mut r.data;
        Ok(Circle {
            center: d.read_3bd()?,
            radius: d.read_bd()?,
            thickness: d.read_bt()?,
            extrusion: d.read_be()?,
        })
    }

    pub(crate) fn encode(&self, w: &mut ObjWriter) -> Result<()> {
        let d = &mut w.data;
        d.write_3bd(self.center);
        d.write_bd(self.radius);
        d.write_bt(self.thickness);
        d.write_be(self.extrusion);
        Ok(())
    }
}

/// An arc (ODS 20.4.18). Angles are in radians.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct Arc {
    pub circle: Circle,
    pub start_angle: f64,
    pub end_angle: f64,
}

impl Arc {
    pub(crate) fn decode(r: &mut ObjReader) -> Result<Self> {
        let circle = Circle::decode(r)?;
        Ok(Arc {
            circle,
            start_angle: r.data.read_bd()?,
            end_angle: r.data.read_bd()?,
        })
    }

    pub(crate) fn encode(&self, w: &mut ObjWriter) -> Result<()> {
        self.circle.encode(w)?;
        w.data.write_bd(self.start_angle);
        w.data.write_bd(self.end_angle);
        Ok(())
    }
}

/// An ellipse (ODS 20.4.39). The center and major axis are in WCS; the
/// angles are eccentric anomalies in radians.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct Ellipse {
    pub center: [f64; 3],
    /// The semi-major axis vector.
    pub major_axis: [f64; 3],
    pub extrusion: [f64; 3],
    /// Minor to major axis ratio.
    pub axis_ratio: f64,
    pub start_angle: f64,
    pub end_angle: f64,
}

impl Ellipse {
    pub(crate) fn decode(r: &mut ObjReader) -> Result<Self> {
        let d = &mut r.data;
        Ok(Ellipse {
            center: d.read_3bd()?,
            major_axis: d.read_3bd()?,
            extrusion: d.read_3bd()?,
            axis_ratio: d.read_bd()?,
            start_angle: d.read_bd()?,
            end_angle: d.read_bd()?,
        })
    }

    pub(crate) fn encode(&self, w: &mut ObjWriter) -> Result<()> {
        let d = &mut w.data;
        d.write_3bd(self.center);
        d.write_3bd(self.major_axis);
        d.write_3bd(self.extrusion);
        d.write_bd(self.axis_ratio);
        d.write_bd(self.start_angle);
        d.write_bd(self.end_angle);
        Ok(())
    }
}

/// A ray (ODS 20.4.42), or an xline (20.4.43, the same layout): a point and
/// a direction.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct Ray {
    pub point: [f64; 3],
    pub vector: [f64; 3],
}

/// An xline: a line infinite in both directions.
pub type Xline = Ray;

impl Ray {
    pub(crate) fn decode(r: &mut ObjReader) -> Result<Self> {
        Ok(Ray {
            point: r.data.read_3bd()?,
            vector: r.data.read_3bd()?,
        })
    }

    pub(crate) fn encode(&self, w: &mut ObjWriter) -> Result<()> {
        w.data.write_3bd(self.point);
        w.data.write_3bd(self.vector);
        Ok(())
    }
}

/// A 2D solid (ODS 20.4.35), or a trace (20.4.36, the same layout). The
/// corners are in OCS at `elevation`.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct Solid {
    pub thickness: f64,
    pub elevation: f64,
    pub corners: [[f64; 2]; 4],
    pub extrusion: [f64; 3],
}

/// A trace: a solid drawn by the TRACE command.
pub type Trace = Solid;

impl Solid {
    pub(crate) fn decode(r: &mut ObjReader) -> Result<Self> {
        let d = &mut r.data;
        let thickness = d.read_bt()?;
        let elevation = d.read_bd()?;
        let mut corners = [[0.0; 2]; 4];
        for c in &mut corners {
            *c = d.read_2rd()?;
        }
        Ok(Solid {
            thickness,
            elevation,
            corners,
            extrusion: d.read_be()?,
        })
    }

    pub(crate) fn encode(&self, w: &mut ObjWriter) -> Result<()> {
        let d = &mut w.data;
        d.write_bt(self.thickness);
        d.write_bd(self.elevation);
        for c in self.corners {
            d.write_2rd(c);
        }
        d.write_be(self.extrusion);
        Ok(())
    }
}

/// A 3D face (ODS 20.4.32).
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct Face3d {
    pub corners: [[f64; 3]; 4],
    /// Invisible edge flags (DXF 70): 1, 2, 4 and 8 for the first to fourth
    /// edge.
    pub invisible_edges: i16,
}

impl Face3d {
    pub(crate) fn decode(r: &mut ObjReader) -> Result<Self> {
        let d = &mut r.data;
        let no_flags = d.read_b()?;
        let z_zero = d.read_b()?;
        let x = d.read_rd()?;
        let y = d.read_rd()?;
        let z = if z_zero { 0.0 } else { d.read_rd()? };
        let mut corners = [[x, y, z]; 4];
        for i in 1..4 {
            let prev = corners[i - 1];
            for (k, c) in corners[i].iter_mut().enumerate() {
                *c = d.read_dd(prev[k])?;
            }
        }
        let invisible_edges = if no_flags { 0 } else { d.read_bs()? };
        Ok(Face3d {
            corners,
            invisible_edges,
        })
    }

    pub(crate) fn encode(&self, w: &mut ObjWriter) -> Result<()> {
        let d = &mut w.data;
        let [first, ..] = self.corners;
        let z_zero = is_zero(first[2]);
        d.write_b(self.invisible_edges == 0);
        d.write_b(z_zero);
        d.write_rd(first[0]);
        d.write_rd(first[1]);
        if !z_zero {
            d.write_rd(first[2]);
        }
        for i in 1..4 {
            for k in 0..3 {
                d.write_dd(self.corners[i][k], self.corners[i - 1][k]);
            }
        }
        if self.invisible_edges != 0 {
            d.write_bs(self.invisible_edges);
        }
        Ok(())
    }
}

/// A shape (ODS 20.4.37): shape `shape_number` of the shape file `style`.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct Shape {
    pub insertion: [f64; 3],
    pub scale: f64,
    pub rotation: f64,
    pub width_factor: f64,
    pub oblique: f64,
    pub thickness: f64,
    pub shape_number: i16,
    pub extrusion: [f64; 3],
    /// The shape file (a STYLE entry; hard pointer).
    pub style: Handle,
}

impl Shape {
    pub(crate) fn decode(r: &mut ObjReader) -> Result<Self> {
        let d = &mut r.data;
        let insertion = d.read_3bd()?;
        let scale = d.read_bd()?;
        let rotation = d.read_bd()?;
        let width_factor = d.read_bd()?;
        let oblique = d.read_bd()?;
        let thickness = d.read_bd()?;
        let shape_number = d.read_bs()?;
        let extrusion = d.read_3bd()?;
        Ok(Shape {
            insertion,
            scale,
            rotation,
            width_factor,
            oblique,
            thickness,
            shape_number,
            extrusion,
            style: r.href_abs()?,
        })
    }

    pub(crate) fn encode(&self, w: &mut ObjWriter) -> Result<()> {
        let d = &mut w.data;
        d.write_3bd(self.insertion);
        d.write_bd(self.scale);
        d.write_bd(self.rotation);
        d.write_bd(self.width_factor);
        d.write_bd(self.oblique);
        d.write_bd(self.thickness);
        d.write_bs(self.shape_number);
        d.write_3bd(self.extrusion);
        put(w, HARD_POINTER, self.style)
    }
}

/// The BLOCK entity that starts a block definition (ODS 20.4.6).
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(missing_docs)] // field by field, as ODS names them
pub struct Block {
    pub name: String,
}

impl Block {
    pub(crate) fn decode(r: &mut ObjReader) -> Result<Self> {
        Ok(Block { name: r.text()? })
    }

    pub(crate) fn encode(&self, w: &mut ObjWriter) -> Result<()> {
        w.text(&self.name)
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::roundtrip;
    use super::*;
    use crate::object::type_code;

    #[test]
    fn roundtrip_line() {
        let mut l = Line {
            start: [1.5, -2.0, 3.0],
            end: [1.5, 7.25, -3.0],
            thickness: 0.5,
            extrusion: [0.0, 0.6, 0.8],
        };
        roundtrip(&l, Line::encode, Line::decode, type_code::LINE);
        // The zero-Z form, BT 0 and BE (0,0,1).
        l.start[2] = 0.0;
        l.end[2] = 0.0;
        l.thickness = 0.0;
        l.extrusion = [0.0, 0.0, 1.0];
        roundtrip(&l, Line::encode, Line::decode, type_code::LINE);
        // A stored -0.0 Z is not the zero form.
        l.end[2] = -0.0;
        roundtrip(&l, Line::encode, Line::decode, type_code::LINE);
    }

    #[test]
    fn roundtrip_point() {
        let p = Point {
            position: [1.0, 2.0, 3.0],
            thickness: 2.0,
            extrusion: [1.0, 0.0, 0.0],
            x_axis_angle: 0.25,
        };
        roundtrip(&p, Point::encode, Point::decode, type_code::POINT);
    }

    fn circle() -> Circle {
        Circle {
            center: [10.0, 20.0, 0.5],
            radius: 4.5,
            thickness: 1.0,
            extrusion: [0.0, 0.0, -1.0],
        }
    }

    #[test]
    fn roundtrip_circle() {
        roundtrip(&circle(), Circle::encode, Circle::decode, type_code::CIRCLE);
    }

    #[test]
    fn roundtrip_arc() {
        let a = Arc {
            circle: circle(),
            start_angle: 0.5,
            end_angle: 3.0,
        };
        roundtrip(&a, Arc::encode, Arc::decode, type_code::ARC);
    }

    #[test]
    fn roundtrip_ellipse() {
        let e = Ellipse {
            center: [1.0, 2.0, 3.0],
            major_axis: [4.0, 0.0, 0.0],
            extrusion: [0.0, 0.0, 1.0],
            axis_ratio: 0.5,
            start_angle: 0.1,
            end_angle: 6.0,
        };
        roundtrip(&e, Ellipse::encode, Ellipse::decode, type_code::ELLIPSE);
    }

    #[test]
    fn roundtrip_ray_and_xline() {
        let r = Ray {
            point: [1.0, 2.0, 3.0],
            vector: [0.6, 0.8, 0.0],
        };
        roundtrip(&r, Ray::encode, Ray::decode, type_code::RAY);
        roundtrip(&r, Xline::encode, Xline::decode, type_code::XLINE);
    }

    #[test]
    fn roundtrip_solid_and_trace() {
        let s = Solid {
            thickness: 1.5,
            elevation: 2.0,
            corners: [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [1.0, 1.0]],
            extrusion: [0.0, 1.0, 0.0],
        };
        roundtrip(&s, Solid::encode, Solid::decode, type_code::SOLID);
        roundtrip(&s, Trace::encode, Trace::decode, type_code::TRACE);
    }

    #[test]
    fn roundtrip_face3d() {
        let mut f = Face3d {
            corners: [
                [0.0, 0.0, 1.0],
                [5.0, 0.0, 1.0],
                [5.0, 5.0, 1.0],
                [5.0, 5.0, 1.0],
            ],
            invisible_edges: 0b1010,
        };
        roundtrip(&f, Face3d::encode, Face3d::decode, type_code::FACE3D);
        // No flags, first Z zero.
        f.invisible_edges = 0;
        f.corners[0][2] = 0.0;
        roundtrip(&f, Face3d::encode, Face3d::decode, type_code::FACE3D);
    }

    #[test]
    fn roundtrip_shape() {
        let s = Shape {
            insertion: [1.0, 2.0, 0.0],
            scale: 2.5,
            rotation: 0.3,
            width_factor: 0.9,
            oblique: 0.1,
            thickness: 0.2,
            shape_number: 130,
            extrusion: [0.0, 0.0, 1.0],
            style: Handle(0x27F),
        };
        roundtrip(&s, Shape::encode, Shape::decode, type_code::SHAPE);
    }

    #[test]
    fn roundtrip_block() {
        let b = Block {
            name: "*U12 ブロック".into(),
        };
        roundtrip(&b, Block::encode, Block::decode, type_code::BLOCK);
    }
}
