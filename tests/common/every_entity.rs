//! One entity of every kind the library models, added to a drawing through
//! the public API (Task 24's `new_with_every_entity_kind`, shared with the
//! Task 26 sign-off pack and oracle tests).

use opendwg::bits::Color;
use opendwg::object::{
    ATTRIB_SINGLE_LINE, Arc, AsRead, AttDef, Attrib, BoundaryPath, CLIP_RECTANGLE, Circle,
    Dictionary, DimCommon, DimKind, Dimension, Ellipse, Entity, EntityCommon, EntityList, Face3d,
    Hatch, HatchEdge, HatchGradient, HatchPattern, HatchVertex, Image, ImageDef, Insert,
    LWPOLYLINE_CLOSED, Leader, LeaderLine, LeaderRoot, Line, LwPolyline, MInsert, MLeaderContent,
    MLeaderContext, MLeaderText, MLine, MLineElement, MLineVertex, MText, MultiLeader,
    NonGraphical, ORDINATE_X, Object, Ole2Frame, PATH_POLYLINE, PathEdges, Point, PolyfaceMesh,
    PolygonMesh, Polyline2d, Polyline3d, PolylinePath, RasterVariables, Ray,
    SPLINE_FLAGS1_METHOD_FIT, SPLINE_FLAGS1_USE_KNOT_PARAMETER, SPLINE_KNOT_CHORD,
    SPLINE_KNOT_CUSTOM, Shape, Solid, Spline, SplineControl, SplineFit, SplineForm, TableEntry,
    Text, TextStyle, Tolerance, Vertex2d, Vertex3d, VertexPfaceFace, Viewport, Xline,
};
use opendwg::{Drawing, Handle};

pub fn dictionary_entries(d: &Drawing, h: Handle) -> Vec<(String, Handle)> {
    match d.get(h) {
        Some(Object::Other {
            object: NonGraphical::Dictionary(x),
            ..
        }) => x.entries.clone(),
        Some(Object::Other {
            object: NonGraphical::DictionaryWithDefault(x),
            ..
        }) => x.dictionary.entries.clone(),
        other => panic!("{h} is not a dictionary: {other:?}"),
    }
}

pub fn entry(d: &Drawing, dict: Handle, name: &str) -> Handle {
    dictionary_entries(d, dict)
        .into_iter()
        .find(|(n, _)| n == name)
        .unwrap_or_else(|| panic!("{dict} has no entry {name}"))
        .1
}

// ---- one entity of every kind ----

pub const Z: [f64; 3] = [0.0, 0.0, 1.0];

pub fn line(a: [f64; 3], b: [f64; 3]) -> Entity {
    Entity::Line(Line {
        start: a,
        end: b,
        thickness: 0.0,
        extrusion: Z,
    })
}

pub fn text(value: &str, at: [f64; 3], style: Handle) -> Text {
    Text {
        insertion: at,
        alignment: None,
        extrusion: Z,
        thickness: 0.0,
        oblique_angle: 0.0,
        rotation: 0.0,
        height: 0.2,
        width_factor: 1.0,
        value: value.into(),
        generation: 0,
        horizontal_alignment: 0,
        vertical_alignment: 0,
        style,
    }
}

pub fn attrib(tag: &str, value: &str, at: [f64; 3], style: Handle) -> Attrib {
    Attrib {
        text: text(value, at, style),
        version: 0,
        attribute_type: ATTRIB_SINGLE_LINE,
        mtext: None,
        tag: tag.into(),
        field_length: 0,
        flags: 0,
        lock_position: false,
    }
}

pub fn mtext(contents: &str, at: [f64; 3], style: Handle) -> MText {
    MText {
        insertion: at,
        extrusion: Z,
        x_axis_dir: [1.0, 0.0, 0.0],
        rect_width: 4.0,
        rect_height: 0.0,
        text_height: 0.2,
        attachment: 1,
        drawing_dir: 1,
        extents_height: 0.2,
        extents_width: 3.0,
        contents: contents.into(),
        style,
        line_spacing_style: 1,
        line_spacing_factor: 1.0,
        unknown_bit: false,
        background_flags: 0,
        background: None,
        context: None,
    }
}

pub fn dim_common(style: Handle) -> DimCommon {
    DimCommon {
        class_version: 0,
        extrusion: Z,
        text_midpoint: [5.0, 5.0],
        elevation: 0.0,
        flags: 0,
        user_text: String::new(),
        text_rotation: 0.0,
        horiz_dir: 0.0,
        ins_scale: [1.0; 3],
        ins_rotation: 0.0,
        attachment: 5,
        line_spacing_style: 1,
        line_spacing_factor: 1.0,
        measurement: 0.0,
        unknown_73: false,
        flip_arrows: [false; 2],
        clone_point: [0.0, 0.0],
        dimstyle: style,
        block: Handle(0),
    }
}

pub fn dimension(style: Handle, kind: DimKind) -> Entity {
    let mut d = Dimension {
        common: dim_common(style),
        kind,
        read: AsRead::default(),
    };
    // The measurement written is the one the geometry gives.
    d.common.measurement = d.measurement();
    Entity::Dimension(Box::new(d))
}

pub fn vertex3d(flags: u8, point: [f64; 3]) -> Entity {
    let v = Vertex3d { flags, point };
    match flags {
        32 => Entity::Vertex3d(v),
        64 => Entity::VertexMesh(v),
        _ => Entity::VertexPface(v),
    }
}

pub fn hatch(pattern_name: &str, pattern_type: i16, pattern: Option<HatchPattern>) -> Entity {
    let square = PolylinePath {
        has_bulges: false,
        closed: true,
        vertices: [[0.0, 0.0], [4.0, 0.0], [4.0, 4.0], [0.0, 4.0]]
            .into_iter()
            .map(|point| HatchVertex { point, bulge: 0.0 })
            .collect(),
    };
    let circle = HatchEdge::CircularArc {
        center: [2.0, 2.0],
        radius: 1.0,
        start_angle: 0.0,
        end_angle: std::f64::consts::TAU,
        ccw: true,
    };
    Entity::Hatch(Box::new(Hatch {
        gradient: HatchGradient {
            enabled: 0,
            reserved: 0,
            angle: 0.0,
            shift: 0.0,
            single_color: 0,
            tint: 0.0,
            colors: Vec::new(),
            name: String::new(),
        },
        elevation: 0.0,
        extrusion: Z,
        pattern_name: pattern_name.into(),
        associative: false,
        paths: vec![
            BoundaryPath {
                flags: 1 | PATH_POLYLINE,
                edges: PathEdges::Polyline(square),
                boundary_objects: Vec::new(),
            },
            BoundaryPath {
                flags: 0x10,
                edges: PathEdges::Edges(vec![circle]),
                boundary_objects: Vec::new(),
            },
        ],
        style: 0,
        pattern_type,
        pattern,
        pixel_size: 0.0,
        seed_points: vec![[0.5, 0.5]],
        read_pattern: AsRead::default(),
    }))
}

pub fn viewport(center: [f64; 3], width: f64, height: f64) -> Viewport {
    Viewport {
        center,
        width,
        height,
        view_target: [0.0; 3],
        view_direction: Z,
        twist_angle: 0.0,
        view_height: 9.0,
        lens_length: 50.0,
        front_clip: 0.0,
        back_clip: 0.0,
        snap_angle: 0.0,
        view_center: [6.0, 4.5],
        snap_base: [0.0, 0.0],
        snap_spacing: [0.5, 0.5],
        grid_spacing: [0.5, 0.5],
        circle_zoom: 1000,
        grid_major: 5,
        status_flags: 0x8060,
        style_sheet: String::new(),
        render_mode: 0,
        ucs_at_origin: true,
        ucs_per_viewport: true,
        ucs_origin: [0.0; 3],
        ucs_x_axis: [1.0, 0.0, 0.0],
        ucs_y_axis: [0.0, 1.0, 0.0],
        ucs_elevation: 0.0,
        ucs_ortho_type: 0,
        shade_plot_mode: 0,
        use_default_lights: true,
        default_lighting_type: 1,
        brightness: 0.0,
        contrast: 0.0,
        ambient_color: Color::Index(250),
        frozen_layers: Vec::new(),
        clip_boundary: Handle(0),
        vp_ent_header: None,
        named_ucs: Handle(0),
        base_ucs: Handle(0),
        background: Handle(0),
        visual_style: Handle(0),
        shade_plot: Handle(0),
        sun: Handle(0),
    }
}

pub fn image(insertion: [f64; 3], image_def: Handle) -> Image {
    Image {
        class_version: 0,
        insertion,
        u_vector: [0.01, 0.0, 0.0],
        v_vector: [0.0, 0.01, 0.0],
        size: [200.0, 100.0],
        display_flags: 7,
        clipping: false,
        brightness: 50,
        contrast: 50,
        fade: 0,
        clip_inside: false,
        clip_type: CLIP_RECTANGLE,
        clip_boundary: vec![[-0.5, -0.5], [199.5, 99.5]],
        image_def,
        reactor: Handle(0),
    }
}

pub fn multileader(style: Handle, text_style: Handle) -> MultiLeader {
    let line = LeaderLine {
        points: vec![[0.0, 0.0, 0.0]],
        breaks: Vec::new(),
        index: 0,
        leader_type: 1,
        color: Color::ByBlock,
        line_type: Handle(0),
        line_weight: -2,
        arrow_size: 0.18,
        arrow: Handle(0),
        override_flags: 0,
    };
    let root = LeaderRoot {
        content_valid: true,
        unknown: true,
        connection: [2.0, 2.0, 0.0],
        direction: [1.0, 0.0, 0.0],
        breaks: Vec::new(),
        index: 0,
        landing_distance: 0.36,
        lines: vec![line],
        attachment_direction: 0,
    };
    let content = MLeaderText {
        label: "Note".into(),
        normal: Z,
        style: text_style,
        location: [2.45, 2.09, 0.0],
        direction: [1.0, 0.0, 0.0],
        rotation: 0.0,
        boundary_width: 0.0,
        boundary_height: 0.0,
        line_spacing_factor: 1.0,
        line_spacing_style: 1,
        color: Color::ByBlock,
        alignment: 1,
        flow_direction: 1,
        bg_color: Color::Index(7),
        bg_scale: 1.5,
        bg_transparency: 0,
        bg_fill: false,
        bg_mask_fill: false,
        column_type: 0,
        auto_height: false,
        column_width: 0.0,
        column_gutter: 0.0,
        column_flow_reversed: false,
        column_sizes: Vec::new(),
        word_break: true,
        unknown: false,
    };
    MultiLeader {
        version: 2,
        context: MLeaderContext {
            roots: vec![root],
            scale: 1.0,
            content_base: [2.45, 2.0, 0.0],
            text_height: 0.18,
            arrowhead_size: 0.18,
            landing_gap: 0.09,
            text_left_attachment: 1,
            text_right_attachment: 1,
            text_align: 0,
            block_connection: 0,
            content: MLeaderContent::Text(Box::new(content)),
            base_point: [0.0; 3],
            base_direction: [1.0, 0.0, 0.0],
            base_vertical: [0.0, 1.0, 0.0],
            normal_reversed: false,
            text_top_attachment: 9,
            text_bottom_attachment: 9,
        },
        style,
        override_flags: 0,
        leader_type: 1,
        line_color: Color::ByBlock,
        line_type: Handle(0),
        line_weight: -2,
        landing_enabled: true,
        dogleg_enabled: true,
        landing_distance: 0.36,
        arrowhead: Handle(0),
        arrowhead_size: 0.18,
        content_type: 2,
        text_style,
        text_left_attachment: 1,
        text_right_attachment: 1,
        text_angle_type: 1,
        text_alignment: 0,
        text_color: Color::ByBlock,
        text_frame: false,
        block: Handle(0),
        block_color: Color::ByBlock,
        block_scale: [1.0; 3],
        block_rotation: 0.0,
        block_connection: 0,
        annotative: false,
        arrowheads: Vec::new(),
        block_labels: Vec::new(),
        text_direction_negative: false,
        ipe_align: 0,
        justification: 1,
        scale: 1.0,
        attachment_direction: 0,
        text_bottom_attachment: 9,
        text_top_attachment: 9,
        extended_to_text: false,
        read_content: AsRead::default(),
    }
}

/// `d` with one entity of every kind the library models (ACIS entities
/// aside: without data they need a data storage record) added through the
/// public API, with the objects they use (a block with attribute
/// definitions, a shape file, an image definition with its dictionary and
/// raster variables, a dimension style's dimensions, a multiline style).
pub fn with_every_entity(d: Drawing) -> (Drawing, EveryEntity) {
    let mut d = d;
    let ms = d.model_space();
    let ps = d.header.block_record_paper_space;
    let l0 = d.layer_by_name("0").unwrap();
    let c = || EntityCommon::new(l0);
    let standard = d.header.textstyle;
    let dimstyle = d.header.dimstyle;
    let add = |d: &mut Drawing, block: Handle, e: Entity| d.add_entity(block, c(), e).unwrap();

    // Simple entities.
    add(&mut d, ms, line([0.0; 3], [10.0, 5.0, 0.0]));
    add(
        &mut d,
        ms,
        Entity::Point(Point {
            position: [1.0, 1.0, 0.0],
            thickness: 0.0,
            extrusion: Z,
            x_axis_angle: 0.0,
        }),
    );
    let circle = Circle {
        center: [3.0, 3.0, 0.0],
        radius: 1.5,
        thickness: 0.0,
        extrusion: Z,
    };
    add(&mut d, ms, Entity::Circle(circle.clone()));
    add(
        &mut d,
        ms,
        Entity::Arc(Arc {
            circle,
            start_angle: 0.0,
            end_angle: 1.5,
        }),
    );
    add(
        &mut d,
        ms,
        Entity::Ellipse(Ellipse {
            center: [5.0, 5.0, 0.0],
            major_axis: [2.0, 0.0, 0.0],
            extrusion: Z,
            axis_ratio: 0.5,
            start_angle: 0.0,
            end_angle: std::f64::consts::TAU,
        }),
    );
    add(
        &mut d,
        ms,
        Entity::Ray(Ray {
            point: [0.0; 3],
            vector: [1.0, 0.0, 0.0],
        }),
    );
    add(
        &mut d,
        ms,
        Entity::Xline(Xline {
            point: [0.0; 3],
            vector: [0.0, 1.0, 0.0],
        }),
    );
    let quad = Solid {
        thickness: 0.0,
        elevation: 0.0,
        corners: [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [1.0, 1.0]],
        extrusion: Z,
    };
    add(&mut d, ms, Entity::Solid(quad.clone()));
    add(&mut d, ms, Entity::Trace(quad));
    add(
        &mut d,
        ms,
        Entity::Face3d(Face3d {
            corners: [[0.0; 3], [1.0, 0.0, 0.0], [1.0, 1.0, 1.0], [0.0, 1.0, 1.0]],
            invisible_edges: 0,
        }),
    );
    add(
        &mut d,
        ms,
        Entity::Text(text("Hello", [0.0, 6.0, 0.0], standard)),
    );
    add(
        &mut d,
        ms,
        Entity::MText(Box::new(mtext(
            "Line one\\PLine two \\U+00E9 漢字",
            [0.0, 8.0, 0.0],
            standard,
        ))),
    );
    add(
        &mut d,
        ms,
        Entity::LwPolyline(Box::new(LwPolyline {
            flags: LWPOLYLINE_CLOSED,
            const_width: 0.0,
            elevation: 0.0,
            thickness: 0.0,
            extrusion: Z,
            points: vec![[0.0, 0.0], [2.0, 0.0], [2.0, 1.0]],
            bulges: vec![0.0, 0.5, 0.0],
            vertex_ids: Vec::new(),
            widths: Vec::new(),
            read_points: AsRead::default(),
        })),
    );

    // A shape needs a shape file (an unnamed STYLE).
    let shapes = d
        .add_text_style(TextStyle {
            entry: TableEntry::default(),
            vertical: false,
            shape_file: true,
            fixed_height: 0.0,
            width_factor: 1.0,
            oblique_angle: 0.0,
            generation: 0,
            last_height: 0.2,
            font_name: "ltypeshp.shx".into(),
            bigfont_name: String::new(),
        })
        .unwrap();
    add(
        &mut d,
        ms,
        Entity::Shape(Shape {
            insertion: [7.0, 7.0, 0.0],
            scale: 1.0,
            rotation: 0.0,
            width_factor: 1.0,
            oblique: 0.0,
            thickness: 0.0,
            shape_number: 132,
            extrusion: Z,
            style: shapes,
        }),
    );

    // A block with an attribute definition, inserted with an attribute;
    // also as a MINSERT.
    let block = d.add_block("TITLE").unwrap();
    add(&mut d, block, line([0.0; 3], [1.0, 0.0, 0.0]));
    add(
        &mut d,
        block,
        Entity::AttDef(Box::new(AttDef {
            attrib: attrib("NAME", "default", [0.0, 0.5, 0.0], standard),
            prompt_version: 0,
            prompt: "Name?".into(),
        })),
    );
    let insert = Insert {
        insertion: [10.0, 10.0, 0.0],
        scale: [1.0; 3],
        rotation: 0.0,
        extrusion: Z,
        block,
        attribs: EntityList::Owned(Vec::new()),
        seqend: None,
    };
    let with_attribs = |at: [f64; 3]| vec![(c(), attrib("NAME", "Drawing 1", at, standard))];
    d.add_insert_with_attribs(
        ms,
        (c(), Entity::Insert(insert.clone())),
        with_attribs([10.0, 10.5, 0.0]),
    )
    .unwrap();
    let minsert = MInsert {
        insert: Insert {
            insertion: [20.0, 10.0, 0.0],
            ..insert
        },
        columns: 2,
        rows: 3,
        column_spacing: 2.0,
        row_spacing: 1.0,
    };
    d.add_insert_with_attribs(
        ms,
        (c(), Entity::MInsert(minsert)),
        with_attribs([20.0, 10.5, 0.0]),
    )
    .unwrap();

    // Polylines of every kind.
    let v2d = |x: f64, y: f64| {
        Entity::Vertex2d(Vertex2d {
            flags: 0,
            point: [x, y, 0.0],
            start_width: 0.0,
            end_width: 0.0,
            bulge: 0.0,
            id: 0,
            tangent_dir: 0.0,
        })
    };
    let poly2d = Polyline2d {
        flags: 1,
        curve_type: 0,
        start_width: 0.0,
        end_width: 0.0,
        thickness: 0.0,
        elevation: 0.0,
        extrusion: Z,
        vertices: EntityList::Owned(Vec::new()),
        seqend: Handle(0),
    };
    let verts = vec![
        (c(), v2d(0.0, 0.0)),
        (c(), v2d(1.0, 0.0)),
        (c(), v2d(1.0, 1.0)),
    ];
    d.add_polyline(ms, (c(), Entity::Polyline2d(poly2d)), verts)
        .unwrap();
    let poly3d = Polyline3d {
        spline_flags: 0,
        closed_flags: 0,
        vertices: EntityList::Owned(Vec::new()),
        seqend: Handle(0),
    };
    let verts = vec![
        (c(), vertex3d(32, [0.0; 3])),
        (c(), vertex3d(32, [1.0, 1.0, 1.0])),
    ];
    d.add_polyline(ms, (c(), Entity::Polyline3d(poly3d)), verts)
        .unwrap();
    let pface = PolyfaceMesh {
        vertex_count: 4,
        face_count: 1,
        vertices: EntityList::Owned(Vec::new()),
        seqend: Handle(0),
    };
    let mut verts: Vec<(EntityCommon, Entity)> = [
        [0.0, 0.0, 0.0],
        [1.0, 0.0, 0.0],
        [1.0, 1.0, 0.0],
        [0.0, 1.0, 0.0],
    ]
    .into_iter()
    .map(|p| (c(), vertex3d(192, p)))
    .collect();
    verts.push((
        c(),
        Entity::VertexPfaceFace(VertexPfaceFace {
            indices: [1, 2, 3, 4],
        }),
    ));
    d.add_polyline(ms, (c(), Entity::PolyfaceMesh(pface)), verts)
        .unwrap();
    let mesh = PolygonMesh {
        flags: 16,
        curve_type: 0,
        m_count: 2,
        n_count: 2,
        m_density: 0,
        n_density: 0,
        vertices: EntityList::Owned(Vec::new()),
        seqend: Handle(0),
    };
    let verts = [
        [0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [1.0, 0.0, 0.0],
        [1.0, 1.0, 1.0],
    ]
    .into_iter()
    .map(|p| (c(), vertex3d(64, p)))
    .collect();
    d.add_polyline(ms, (c(), Entity::PolygonMesh(mesh)), verts)
        .unwrap();

    // Splines in both forms.
    add(
        &mut d,
        ms,
        Entity::Spline(Box::new(Spline {
            degree: 3,
            flags1: 0,
            knot_parameter: SPLINE_KNOT_CUSTOM,
            form: SplineForm::Control(SplineControl {
                rational: false,
                closed: false,
                periodic: false,
                knot_tol: 1e-10,
                ctrl_tol: 1e-10,
                knots: vec![0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0],
                points: vec![[0.0; 3], [1.0, 2.0, 0.0], [3.0, 2.0, 0.0], [4.0, 0.0, 0.0]],
                weights: None,
            }),
            read: AsRead::default(),
        })),
    );
    add(
        &mut d,
        ms,
        Entity::Spline(Box::new(Spline {
            degree: 3,
            flags1: SPLINE_FLAGS1_METHOD_FIT | SPLINE_FLAGS1_USE_KNOT_PARAMETER,
            knot_parameter: SPLINE_KNOT_CHORD,
            form: SplineForm::Fit(SplineFit {
                fit_tol: 1e-10,
                begin_tangent: [0.0; 3],
                end_tangent: [0.0; 3],
                points: vec![[0.0, 3.0, 0.0], [2.0, 4.0, 0.0], [4.0, 3.0, 0.0]],
            }),
            read: AsRead::default(),
        })),
    );

    // A multiline in the Standard style (two elements).
    let mline_style = d.header.cmlstyle;
    let element = || MLineElement {
        segment_params: vec![0.0, 0.0],
        fill_params: Vec::new(),
    };
    add(
        &mut d,
        ms,
        Entity::MLine(Box::new(MLine {
            scale: 1.0,
            justification: 0,
            base_point: [0.0, 12.0, 0.0],
            extrusion: Z,
            flags: 1,
            lines_in_style: 2,
            vertices: [[0.0, 12.0, 0.0], [5.0, 12.0, 0.0]]
                .into_iter()
                .map(|point| MLineVertex {
                    point,
                    direction: [1.0, 0.0, 0.0],
                    miter: [0.0, 1.0, 0.0],
                    lines: vec![element(), element()],
                })
                .collect(),
            style: mline_style,
        })),
    );

    // Every dimension kind, a leader, a tolerance.
    let p = |x: f64, y: f64| [x, y, 0.0];
    for kind in [
        DimKind::Linear {
            ext1: p(0.0, 0.0),
            ext2: p(4.0, 0.0),
            def_point: p(4.0, 1.0),
            ext_line_rotation: 0.0,
            rotation: 0.0,
        },
        DimKind::Aligned {
            ext1: p(0.0, 0.0),
            ext2: p(3.0, 4.0),
            def_point: p(2.0, 5.0),
            ext_line_rotation: 0.0,
        },
        DimKind::Angular3Pt {
            def_point: p(2.0, 2.0),
            ext1: p(3.0, 0.0),
            ext2: p(0.0, 3.0),
            vertex: p(0.0, 0.0),
        },
        DimKind::Angular2Line {
            arc_point: [2.0, 2.0],
            line1: [p(0.0, 0.0), p(3.0, 0.0)],
            line2: [p(0.0, 0.0), p(0.0, 3.0)],
        },
        DimKind::Radius {
            center: p(3.0, 3.0),
            chord_point: p(4.5, 3.0),
            leader_length: 0.0,
        },
        DimKind::Diameter {
            chord_point: p(4.5, 3.0),
            far_chord_point: p(1.5, 3.0),
            leader_length: 0.0,
        },
        DimKind::Ordinate {
            def_point: p(0.0, 0.0),
            feature_location: p(2.0, 1.0),
            leader_endpoint: p(2.0, 3.0),
            flags2: ORDINATE_X,
        },
        DimKind::ArcLength {
            arc_point: p(3.0, 5.0),
            ext1: p(4.5, 3.0),
            ext2: p(3.0, 4.5),
            center: p(3.0, 3.0),
            is_partial: false,
            start_angle: 0.0,
            end_angle: 1.5,
            has_leader: false,
            leader1: p(0.0, 0.0),
            leader2: p(0.0, 0.0),
        },
        DimKind::LargeRadial {
            center: p(3.0, 3.0),
            chord_point: p(4.5, 3.0),
            unknown_40: 0.0,
            override_center: p(6.0, 6.0),
            jog_point: p(5.0, 5.0),
        },
    ] {
        add(&mut d, ms, dimension(dimstyle, kind));
    }
    add(
        &mut d,
        ms,
        Entity::Leader(Box::new(Leader {
            unknown_bit: false,
            annot_type: 3,
            path_type: 0,
            points: vec![p(0.0, 0.0), p(1.0, 1.0), p(2.0, 1.0)],
            origin: p(0.0, 0.0),
            extrusion: Z,
            x_direction: [1.0, 0.0, 0.0],
            block_offset: [0.0; 3],
            endpoint_proj: [0.0; 3],
            box_size: None,
            hook_line_on_x_dir: false,
            arrowhead_on: true,
            unknown_bs: 0,
            unknown_b: [false, false],
            annotation: Handle(0),
            dimstyle,
        })),
    );
    add(
        &mut d,
        ms,
        Entity::Tolerance(Tolerance {
            insertion: p(8.0, 8.0),
            x_direction: [1.0, 0.0, 0.0],
            extrusion: Z,
            text: "{\\Fgdt;j}%%v0.1".into(),
            dimstyle,
        }),
    );
    let ml_style = entry(
        &d,
        entry(&d, d.named_objects(), "ACAD_MLEADERSTYLE"),
        "Standard",
    );
    let ml = multileader(ml_style, standard);
    add(&mut d, ms, Entity::MultiLeader(Box::new(ml)));

    // Hatches: user-defined lines, and a solid fill.
    let lines = HatchPattern::user_defined_lines(0.785, 0.25, false);
    assert_eq!(lines.len(), 1);
    let user = HatchPattern {
        angle: 0.785,
        scale: 0.25,
        double: false,
        lines,
    };
    add(&mut d, ms, hatch("_USER", 0, Some(user)));
    add(&mut d, ms, hatch("SOLID", 1, None));

    // An image of an image definition (in the named object dictionary's
    // ACAD_IMAGE_DICT, with the raster variables), and a wipeout.
    let nod = d.named_objects();
    let dict = Dictionary {
        entries: Vec::new(),
        cloning: 1,
        hard_owner: 0,
    };
    let images = d
        .add_object(nod, "ACAD_IMAGE_DICT", NonGraphical::Dictionary(dict))
        .unwrap();
    let vars = RasterVariables {
        class_version: 0,
        display_frame: 1,
        display_quality: 1,
        units: 0,
    };
    d.add_object(nod, "ACAD_IMAGE_VARS", NonGraphical::RasterVariables(vars))
        .unwrap();
    let def = ImageDef {
        class_version: 0,
        size: [200.0, 100.0],
        path: "logo.png".into(),
        loaded: true,
        resolution_units: 0,
        pixel_size: [0.01, 0.01],
    };
    let def = d
        .add_object(images, "logo", NonGraphical::ImageDef(def))
        .unwrap();
    let img = add(
        &mut d,
        ms,
        Entity::Image(Box::new(image([12.0, 0.0, 0.0], def))),
    );
    let mut wipeout = image([14.0, 0.0, 0.0], Handle(0));
    wipeout.size = [1.0, 1.0];
    wipeout.clip_boundary = vec![[-0.5, -0.5], [0.5, -0.5], [0.5, 0.5], [-0.5, -0.5]];
    wipeout.clip_type = 2;
    add(&mut d, ms, Entity::Wipeout(Box::new(wipeout)));

    // An OLE frame (its data kept as given), and a paper space viewport.
    add(
        &mut d,
        ms,
        Entity::Ole2Frame(Box::new(Ole2Frame {
            flags: 2,
            mode: 1,
            data: vec![0x55; 64],
            unknown: 0,
        })),
    );
    add(
        &mut d,
        ps,
        Entity::Viewport(Box::new(viewport([5.5, 4.25, 0.0], 9.0, 6.0))),
    );
    (
        d,
        EveryEntity {
            image: img,
            image_def: def,
        },
    )
}

/// Objects of [`with_every_entity`] its callers check.
pub struct EveryEntity {
    /// The IMAGE entity.
    pub image: Handle,
    /// Its IMAGEDEF.
    pub image_def: Handle,
}
