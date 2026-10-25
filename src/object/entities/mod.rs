//! Typed entities. Each type decodes and encodes only its type-specific
//! data; the common entity data is handled by [`super::decode_object`].

pub mod basic;
pub mod dim;
pub mod hatch;
pub mod insert;
pub mod misc;
pub mod mleader;
pub mod poly;
pub mod text;

pub use basic::{
    Arc, Block, Circle, Ellipse, Face3d, Line, Point, Ray, Shape, Solid, Trace, Xline,
};
pub use dim::{DimCommon, DimKind, Dimension, Leader, ORDINATE_X, Tolerance};
pub use hatch::{
    BoundaryPath, GradientColor, Hatch, HatchEdge, HatchGradient, HatchPattern, HatchSpline,
    HatchVertex, PATH_DERIVED, PATH_POLYLINE, PathEdges, PatternLine, PatternSettings,
    PolylinePath,
};
pub use insert::{Insert, MInsert};
pub use misc::{
    AcisEntity, AcisGuid, AcisKind, AcisPayload, CLIP_RECTANGLE, Image, Ole2Frame, ProxyEntity,
    Viewport, decrypt_sat,
};
pub use mleader::{
    ContentKind, LeaderBreak, LeaderLine, LeaderRoot, MLeaderBlock, MLeaderBlockLabel,
    MLeaderContent, MLeaderContext, MLeaderText, MultiLeader,
};
pub use poly::{
    LWPOLYLINE_CLOSED, LWPOLYLINE_PLINEGEN, LwPolyline, MLine, MLineElement, MLineVertex,
    PolyfaceMesh, PolygonMesh, Polyline2d, Polyline3d, SPLINE_FLAGS1_CLOSED,
    SPLINE_FLAGS1_METHOD_FIT, SPLINE_FLAGS1_USE_KNOT_PARAMETER, SPLINE_KNOT_CHORD,
    SPLINE_KNOT_CUSTOM, Spline, SplineAsRead, SplineControl, SplineFit, SplineForm, Vertex2d,
    Vertex3d, VertexMesh, VertexPface, VertexPfaceFace,
};
pub use text::{
    ATTDEF_MULTI_LINE, ATTRIB_MULTI_LINE, ATTRIB_SINGLE_LINE, AnnotativeData, AttDef, Attrib,
    EmbeddedMText, MTEXT_CONTEXT_VERSION, MText, MTextBackground, MTextColumns, MTextContext,
    MTextCopies, Text,
};

#[cfg(test)]
pub(crate) mod tests {
    use crate::error::Result;
    use crate::handle::Handle;
    use crate::object::stream::{ObjReader, ObjWriter};
    use crate::version::Version;

    const H: Handle = Handle(0x80);

    pub(crate) fn reader(bytes: &[u8]) -> ObjReader<'_> {
        let (mut r, _) = ObjReader::split(bytes, Version::R2018, 30).unwrap();
        r.handle = H;
        r
    }

    /// Encode `v` as R2018 type-specific data, decode it and compare; then
    /// cut each stream short at every bit and check the decode fails
    /// cleanly (never panics).
    pub(crate) fn roundtrip<T: PartialEq + std::fmt::Debug>(
        v: &T,
        enc: fn(&T, &mut ObjWriter) -> Result<()>,
        dec: fn(&mut ObjReader) -> Result<T>,
        code: u16,
    ) {
        let mut w = ObjWriter::new(H);
        enc(v, &mut w).unwrap();
        let bytes = w.finish(code).unwrap();
        let mut r = reader(&bytes);
        assert_eq!(&dec(&mut r).unwrap(), v);
        assert_eq!(r.data.remaining_bits(), 0);
        assert_eq!(r.strings.as_ref().unwrap().remaining_bits(), 0);
        assert!(r.handles.remaining_bits() < 8);
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);

        let full = reader(&bytes);
        let lens = [
            full.data.remaining_bits(),
            full.strings.as_ref().unwrap().remaining_bits(),
            full.handles.remaining_bits() / 8 * 8,
        ];
        for (stream, &len) in lens.iter().enumerate() {
            for cut in 0..len {
                let mut r = reader(&bytes);
                match stream {
                    0 => r.data.limit_remaining(cut),
                    1 => r.strings.as_mut().unwrap().limit_remaining(cut),
                    _ => r.handles.limit_remaining(cut),
                }
                assert!(dec(&mut r).is_err(), "stream {stream} cut to {cut} bits");
            }
        }
    }

    /// Whole entities (common data included) with flipped bits or cut short
    /// decode or fail, but never panic.
    #[test]
    fn hostile_entities_never_panic() {
        use super::text::tests::{attrib, mtext};
        use super::*;
        use crate::bits::Color;
        use crate::object::{
            DecodeCtx, Entity, EntityCommon, EntityList, LinetypeRef, MaterialRef, Object,
            ObjectCommon, PlotStyleRef, decode_object, encode_object,
        };
        let common = EntityCommon {
            obj: ObjectCommon {
                handle: H,
                owner: Handle(0x7F),
                owner_relative: true,
                reactors: vec![Handle(2)],
                xdict: None,
                xdata: vec![],
                has_ds_binary_data: false,
            },
            entmode: 0,
            layer: Handle(0x10),
            linetype: LinetypeRef::ByLayer,
            plotstyle: PlotStyleRef::ByLayer,
            material: MaterialRef::ByLayer,
            color: Color::Rgb { r: 1, g: 2, b: 3 },
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
        };
        let insert = Insert {
            insertion: [1.0, 2.0, 3.0],
            scale: [1.0, 2.0, 3.0],
            rotation: 0.5,
            extrusion: [0.0, 0.0, 1.0],
            block: Handle(0x30),
            attribs: EntityList::Owned(vec![Handle(0x81), Handle(0x82)]),
            seqend: Some(Handle(0x83)),
        };
        // A dimension whose stored measurement is the one written.
        let dim_of = |i: usize| dim::tests::measured(dim::tests::kinds()[i].clone());
        let entities = [
            Entity::Line(Line {
                start: [1.0, 2.0, 3.0],
                end: [4.0, 5.0, 6.0],
                thickness: 1.0,
                extrusion: [0.0, 1.0, 0.0],
            }),
            Entity::Face3d(Face3d {
                corners: [[1.0; 3], [2.0; 3], [3.0; 3], [4.0; 3]],
                invisible_edges: 3,
            }),
            Entity::Insert(insert.clone()),
            Entity::MInsert(MInsert {
                insert,
                columns: 2,
                rows: 3,
                column_spacing: 1.0,
                row_spacing: 2.0,
                read_attrib_code: Default::default(),
            }),
            Entity::Attrib(Box::new(attrib())),
            Entity::MText(Box::new(mtext())),
            Entity::LwPolyline(Box::new(poly::tests::lwpolyline())),
            Entity::Polyline2d(poly::tests::polyline_2d()),
            Entity::Spline(Box::new(poly::tests::control_spline())),
            Entity::MLine(Box::new(poly::tests::mline())),
            Entity::Dimension(Box::new(dim_of(1))),
            Entity::Dimension(Box::new(dim_of(4))),
            Entity::Dimension(Box::new(dim_of(7))),
            Entity::Dimension(Box::new(dim_of(8))),
            Entity::Tolerance(Tolerance {
                insertion: [1.0, 2.0, 0.0],
                x_direction: [1.0, 0.0, 0.0],
                extrusion: [0.0, 0.0, 1.0],
                text: "{\\Fgdt;p}%%v".into(),
                dimstyle: Handle(0x27),
            }),
            Entity::Leader(Box::new(Leader {
                unknown_bit: false,
                annot_type: 0,
                path_type: 0,
                points: vec![[1.0; 3], [2.0; 3]],
                origin: [1.0; 3],
                extrusion: [0.0, 0.0, 1.0],
                x_direction: [1.0, 0.0, 0.0],
                block_offset: [0.0; 3],
                endpoint_proj: [0.0; 3],
                box_size: None,
                hook_line_on_x_dir: false,
                arrowhead_on: true,
                unknown_bs: 0,
                unknown_b: [false; 2],
                annotation: Handle(0x90),
                dimstyle: Handle(0x27),
            })),
            Entity::MultiLeader(Box::new(mleader::tests::multileader())),
            Entity::Hatch(Box::new(hatch::tests::hatch())),
            Entity::Viewport(Box::new(misc::tests::viewport())),
            Entity::Image(Box::new(misc::tests::image())),
            Entity::Wipeout(Box::new(misc::tests::image())),
            Entity::Acis(Box::new(misc::tests::acis())),
            Entity::Ole2Frame(Box::new(Ole2Frame {
                flags: 1,
                mode: 2,
                data: vec![1, 2, 3, 4],
                unknown: 0,
            })),
            Entity::ProxyEntity(Box::new(misc::tests::proxy())),
        ];
        // The class types (ARC_DIMENSION, LARGE_RADIAL_DIMENSION, MULTILEADER,
        // IMAGE, WIPEOUT) need their classes.
        let class = |n, name: &str| crate::section::classes::DxfClass {
            class_number: n,
            proxy_flags: 0,
            app_name: "ObjectDBX Classes".into(),
            cpp_class_name: "AcDbThing".into(),
            dxf_name: name.into(),
            was_zombie: false,
            item_class_id: crate::section::classes::ITEM_CLASS_ENTITY,
            instance_count: 0,
            dwg_version: 0,
            maintenance_version: 0,
            unknown: [0; 2],
        };
        let classes = [
            class(500, "MULTILEADER"),
            class(501, "ARC_DIMENSION"),
            class(502, "LARGE_RADIAL_DIMENSION"),
            class(503, "IMAGE"),
            class(504, "WIPEOUT"),
        ];
        for entity in entities {
            let obj = Object::Entity {
                common: common.clone(),
                entity,
            };
            let good = encode_object(&obj, &classes).unwrap();
            let ctx = || DecodeCtx::new(Version::R2018, 30, &classes);
            let mut c = ctx();
            assert_eq!(decode_object(&good, H, &mut c).unwrap(), obj);
            assert!(c.diagnostics.is_empty(), "{:?}", c.diagnostics);
            for i in 0..good.len() {
                for bit in [0x01u8, 0x10, 0x80] {
                    let mut b = good.clone();
                    b[i] ^= bit;
                    let _ = decode_object(&b, H, &mut ctx());
                }
                let _ = decode_object(&good[..i], H, &mut ctx());
            }
        }
    }
}
