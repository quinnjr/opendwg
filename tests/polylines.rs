//! Corpus checks of the polylines, vertices, splines and mlines (Task 15):
//! every entity of these types decodes typed, the counts match the manifest,
//! R2018 entities re-encode bit for bit, older versions convert, and values
//! that pin ODS deviations are asserted on real data.
//!
//! POLYLINE_2D, POLYLINE_MESH, VERTEX_2D and VERTEX_MESH are not in the
//! corpus; their unit tests in `src/object/entities/poly.rs` are hermetic.

mod common;

use std::collections::{BTreeMap, BTreeSet};

use opendwg::object::{
    DecodeCtx, Entity, EntityList, LWPOLYLINE_CLOSED, ObjReader, Object, SPLINE_KNOT_CUSTOM,
    SplineForm, decode_entity_common, decode_object, encode_object,
};
use opendwg::{Drawing, Handle, Version};

/// The Task 15 entity types, by type name, with their manifest count keys.
const TYPES: [(&str, &str); 12] = [
    ("LWPOLYLINE", "lwpolyline_count"),
    ("POLYLINE_2D", "polyline_2d_count"),
    ("POLYLINE_3D", "polyline_3d_count"),
    ("POLYLINE_PFACE", "polyline_pface_count"),
    ("POLYLINE_MESH", "polyline_mesh_count"),
    ("VERTEX_2D", "vertex_2d_count"),
    ("VERTEX_3D", "vertex_3d_count"),
    ("VERTEX_MESH", "vertex_mesh_count"),
    ("VERTEX_PFACE", "vertex_pface_count"),
    ("VERTEX_PFACE_FACE", "vertex_pface_face_count"),
    ("SPLINE", "spline_count"),
    ("MLINE", "mline_count"),
];

fn is_task_type(name: &str) -> bool {
    TYPES.iter().any(|(t, _)| *t == name)
}

fn entity(d: &Drawing, h: u64) -> &Entity {
    match d.get(Handle(h)) {
        Some(Object::Entity { entity, .. }) => entity,
        o => panic!("{h:X}: {o:?}"),
    }
}

#[test]
fn corpus_polylines_decode() {
    for (e, path) in common::fetched_files_or_skip() {
        let name = &e.name;
        let d = Drawing::open(&path).unwrap();
        let fallbacks: Vec<_> = d
            .diagnostics()
            .iter()
            .filter(|x| x.message.contains("kept raw") || x.message.contains("chain"))
            .collect();
        assert!(fallbacks.is_empty(), "{name}: {fallbacks:?}");
        let mut counts: BTreeMap<&str, i64> = BTreeMap::new();
        for (h, obj) in d.objects() {
            let t = obj.type_name();
            let Some((_, key)) = TYPES.iter().find(|(n, _)| *n == t) else {
                continue;
            };
            assert!(obj.raw().is_none(), "{name}: {h} {t} kept raw");
            *counts.entry(key).or_default() += 1;
        }
        for (_, key) in TYPES {
            let want = e
                .expect
                .get(key)
                .and_then(toml::Value::as_integer)
                .unwrap_or_else(|| panic!("{name}: manifest lacks expect.{key}"));
            assert_eq!(counts.get(key).copied().unwrap_or(0), want, "{name}: {key}");
        }
    }
}

#[test]
fn corpus_polylines_r2018_bytes() {
    let mut types = BTreeSet::new();
    for (e, path) in common::fetched_files_for("AC1032") {
        let name = &e.name;
        let bytes = std::fs::read(&path).unwrap();
        let d = Drawing::from_bytes(&bytes).unwrap();
        let originals = common::object_bytes(&bytes);
        // Per type: objects, and those whose bytes differ but decode equal.
        let mut stats: BTreeMap<String, (usize, usize)> = BTreeMap::new();
        for (h, obj) in d.objects() {
            let t = obj.type_name().into_owned();
            if !is_task_type(&t) {
                continue;
            }
            assert!(obj.raw().is_none(), "{name}: {h} {t} kept raw");
            let out =
                encode_object(obj, &d.classes).unwrap_or_else(|err| panic!("{name}: {h}: {err}"));
            let s = stats.entry(t.clone()).or_default();
            s.0 += 1;
            if out == originals[&h] {
                continue;
            }
            let mut ctx = DecodeCtx::new(Version::R2018, 30, &d.classes);
            let back = decode_object(&out, h, &mut ctx).unwrap();
            assert_eq!(&back, obj, "{name}: {h} {t} re-encodes to different values");
            s.1 += 1;
        }
        for (t, (n, noncanonical)) in &stats {
            if *noncanonical > 0 {
                eprintln!("{name}: {t}: {noncanonical} of {n} non-canonical in the source");
            }
            assert!(
                noncanonical * 50 <= *n,
                "{name}: {t}: {noncanonical} of {n} objects not bit-identical"
            );
        }
        types.extend(stats.into_keys());
    }
    // POLYLINE_2D, POLYLINE_MESH, VERTEX_2D and VERTEX_MESH are not in the
    // corpus.
    if !types.is_empty() {
        assert_eq!(types.len(), 8, "{types:?}");
    }
}

/// Every entity of these types read from an older version encodes as R2018
/// and decodes back to the same value (R2000 polylines once their vertex
/// lists are built).
#[test]
fn corpus_polylines_older_versions_convert() {
    let mut seen: BTreeMap<Version, BTreeSet<String>> = BTreeMap::new();
    for (e, path) in common::fetched_files_or_skip() {
        let d = Drawing::open(&path).unwrap();
        let v = d.source_version().unwrap();
        for (h, obj) in d.objects() {
            let t = obj.type_name().into_owned();
            if !is_task_type(&t) {
                continue;
            }
            let out = encode_object(obj, &d.classes)
                .unwrap_or_else(|err| panic!("{}: {h} {t}: {err}", e.name));
            let mut ctx = DecodeCtx::new(Version::R2018, 30, &d.classes);
            let back = decode_object(&out, h, &mut ctx).unwrap();
            assert!(ctx.diagnostics.is_empty(), "{}: {h}", e.name);
            let mut want = obj.clone();
            if let Object::Entity { common, .. } = &mut want {
                common.prev_next = None;
            }
            assert_eq!(back, want, "{}: {h} {t}", e.name);
            seen.entry(v).or_default().insert(t);
        }
    }
    for (v, types) in seen {
        assert_eq!(types.len(), 8, "{v:?}: {types:?}");
    }
}

/// R2000 polylines store their first and last vertex; the drawing turns them
/// into the full lists, which must equal the R2004 file's stored lists.
#[test]
fn corpus_r2000_polyline_vertex_lists() {
    let (Some(r2000), Some(r2004)) = (
        common::corpus_file_or_skip("acadsharp-sample-ac1015"),
        common::corpus_file_or_skip("acadsharp-sample-ac1018"),
    ) else {
        return;
    };
    let a = Drawing::open(&r2000).unwrap();
    let b = Drawing::open(&r2004).unwrap();
    assert!(a.diagnostics().is_empty(), "{:?}", a.diagnostics());
    let lists = |d: &Drawing| -> BTreeMap<Handle, (EntityList, Handle)> {
        d.objects()
            .filter_map(|(h, o)| match o {
                Object::Entity {
                    entity: Entity::Polyline3d(p),
                    ..
                } => Some((h, (p.vertices.clone(), p.seqend))),
                Object::Entity {
                    entity: Entity::PolyfaceMesh(p),
                    ..
                } => Some((h, (p.vertices.clone(), p.seqend))),
                _ => None,
            })
            .collect()
    };
    let (la, lb) = (lists(&a), lists(&b));
    assert_eq!(la.len(), 2);
    assert_eq!(la, lb);
    let range = |from: u64, to: u64| EntityList::Owned((from..=to).map(Handle).collect());
    // DXF: POLYLINE 42B (70 = 8) owns VERTEXs 42C-430 and SEQEND 431;
    // POLYLINE 422 (70 = 64) owns VERTEXs 423-429 (5 locations, 2 faces)
    // and SEQEND 42A.
    assert_eq!(la[&Handle(0x42B)], (range(0x42C, 0x430), Handle(0x431)));
    assert_eq!(la[&Handle(0x422)], (range(0x423, 0x429), Handle(0x42A)));
}

/// The reference codes of a polyline's R2004+ vertex handles, read from the
/// entity's original bytes (after its common data and type fields).
fn vertex_ref_codes(bytes: &[u8], v: Version, type_bits: impl Fn(&mut ObjReader)) -> Vec<u8> {
    let (mut r, _) = ObjReader::split(bytes, v, 30).unwrap();
    decode_entity_common(&mut r).unwrap();
    type_bits(&mut r);
    let n = r.data.read_bl().unwrap();
    (0..=n).map(|_| r.href().unwrap().code).collect()
}

/// Values of the sample drawing's polylines, splines and mlines, read from
/// every version, against its DXF exports (`sample_AC10xx_ascii.dxf`).
#[test]
fn corpus_polyline_values() {
    let mut seen = 0;
    for (e, path) in common::fetched_files_or_skip() {
        if !e.name.starts_with("acadsharp-sample-") {
            continue;
        }
        seen += 1;
        let name = &e.name;
        let bytes = std::fs::read(&path).unwrap();
        let d = Drawing::from_bytes(&bytes).unwrap();
        let v = d.source_version().unwrap();

        // LWPOLYLINE 4A6: constant width 0.15 (43), two points, open.
        let Entity::LwPolyline(p) = entity(&d, 0x4A6) else {
            panic!("{name}: 4A6");
        };
        assert_eq!(p.const_width, 0.15, "{name}");
        assert_eq!(p.points, [[-0.5, -0.5], [0.5, 0.5]], "{name}");
        assert!(!p.closed() && p.bulges.is_empty() && p.widths.is_empty());
        // LWPOLYLINE 2E4: closed (70 = 1, stored as 0x200, not ODS's bit 1
        // which flags a stored normal), with bulges (42).
        let Entity::LwPolyline(p) = entity(&d, 0x2E4) else {
            panic!("{name}: 2E4");
        };
        assert!(p.closed(), "{name}");
        assert_eq!(p.flags, LWPOLYLINE_CLOSED, "{name}");
        assert_eq!(p.extrusion, [0.0, 0.0, 1.0], "{name}");
        assert_eq!((p.points.len(), p.bulges.len()), (7, 7), "{name}");
        assert_eq!(p.bulges[1], -0.7995594967893557, "{name}");
        // LWPOLYLINE 2E5: bulges on the first two of five points and widths
        // (40/41) only on the last.
        let Entity::LwPolyline(p) = entity(&d, 0x2E5) else {
            panic!("{name}: 2E5");
        };
        assert_eq!(p.bulges.len(), 5, "{name}");
        assert_eq!(p.bulges[0], 2.127776220378056, "{name}");
        assert_eq!(&p.bulges[2..], [0.0; 3], "{name}");
        assert_eq!(p.widths.len(), 5, "{name}");
        assert_eq!(p.widths[..4], [[0.0; 2]; 4], "{name}");
        let [s, w] = p.widths[4];
        assert!((s - 3.669609517619411).abs() < 1e-12 && (w - 5.080563928030537).abs() < 1e-12);
        // LWPOLYLINE 35B: widths, no bulges.
        let Entity::LwPolyline(p) = entity(&d, 0x35B) else {
            panic!("{name}: 35B");
        };
        assert!(p.bulges.is_empty(), "{name}");
        assert_eq!(
            (p.widths[0], p.widths[4]),
            ([0.2, 0.1], [0.1, 0.5]),
            "{name}"
        );
        let [x, y] = p.points[0];
        assert!((x - 12.14051619543163).abs() < 1e-12, "{name}");
        assert!((y - 7.499316163680425).abs() < 1e-12, "{name}");

        // POLYLINE 422 (polyface): 71 = 5 vertices, 72 = 2 faces; faces
        // 428 (1 2 3 -4) and 429 (-1 4 5 0); locations flagged 192.
        let Entity::PolyfaceMesh(p) = entity(&d, 0x422) else {
            panic!("{name}: 422");
        };
        assert_eq!((p.vertex_count, p.face_count), (5, 2), "{name}");
        let Entity::VertexPfaceFace(f) = entity(&d, 0x428) else {
            panic!("{name}: 428");
        };
        assert_eq!(f.indices, [1, 2, 3, -4], "{name}");
        let Entity::VertexPfaceFace(f) = entity(&d, 0x429) else {
            panic!("{name}: 429");
        };
        assert_eq!(f.indices, [-1, 4, 5, 0], "{name}");
        let Entity::VertexPface(p) = entity(&d, 0x423) else {
            panic!("{name}: 423");
        };
        assert_eq!((p.flags, p.point[0]), (192, 213.69202898487106), "{name}");
        // POLYLINE 42B (3D, 70 = 8: not closed, not splined); VERTEX 42D
        // (70 = 32) has Z 5.4786...
        let Entity::Polyline3d(p) = entity(&d, 0x42B) else {
            panic!("{name}: 42B");
        };
        assert_eq!((p.spline_flags, p.closed_flags), (0, 0), "{name}");
        let Entity::Vertex3d(p) = entity(&d, 0x42D) else {
            panic!("{name}: 42D");
        };
        assert_eq!(p.flags, 32, "{name}");
        assert_eq!(p.point[2], 5.47860020423747, "{name}");

        // From R2004 a polyface mesh's vertices are hard owners (code 3),
        // as a 3D polyline's are; ODS gives soft pointers.
        if v >= Version::R2004 {
            let original = common::object_bytes(&bytes);
            let pface = vertex_ref_codes(&original[&Handle(0x422)], v, |r| {
                r.data.read_bs().unwrap();
                r.data.read_bs().unwrap();
            });
            assert_eq!(pface, [3; 8], "{name}");
            let p3d = vertex_ref_codes(&original[&Handle(0x42B)], v, |r| {
                r.data.read_rc().unwrap();
                r.data.read_rc().unwrap();
            });
            assert_eq!(p3d, [3; 6], "{name}");
        }

        // SPLINE 433: control points (DXF 72 = 8 knots, 73 = 4 points), knot
        // tolerance 1e-9, control tolerance 1e-10, degree 3.
        let Entity::Spline(s) = entity(&d, 0x433) else {
            panic!("{name}: 433");
        };
        assert_eq!(s.degree, 3, "{name}");
        assert_eq!((s.flags1, s.knot_parameter), (0, SPLINE_KNOT_CUSTOM));
        let SplineForm::Control(c) = &s.form else {
            panic!("{name}: 433 has fit data");
        };
        assert_eq!(c.knots, [0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0], "{name}");
        assert_eq!((c.knot_tol, c.ctrl_tol), (1e-9, 1e-10), "{name}");
        assert_eq!(c.points.len(), 4, "{name}");
        assert_eq!(c.points[0], [250.45879078322412, 1.157147022223159, 0.0]);
        assert_eq!(c.weights, None, "{name}");
        assert!(!c.rational && !c.closed && !c.periodic, "{name}");
        // SPLINE 434: from R2013 it stores fit data (two fit points, the
        // first and last control points of the DXF export), with flags 1 = 9
        // (fit points, use knot parameter) and uniform parameterization (2),
        // while its scenario field stays 1: the layout follows the knot
        // parameter and flags 1 (as ACadSharp reads it), not the scenario.
        // Older versions store control points and the parameterization in
        // ACAD_SPLINE_DATA extended data.
        let Entity::Spline(s) = entity(&d, 0x434) else {
            panic!("{name}: 434");
        };
        match &s.form {
            SplineForm::Fit(f) if v >= Version::R2013 => {
                assert_eq!((s.flags1, s.knot_parameter), (9, 2), "{name}");
                assert_eq!(s.stored_scenario(), Some(1), "{name}");
                assert_eq!(f.fit_tol, 1e-10, "{name}");
                assert_eq!(
                    f.points,
                    [
                        [250.45879078322412, 17.426024184844767, 0.0],
                        [259.2245511100041, 27.28750455247221, 0.0]
                    ],
                    "{name}"
                );
            }
            SplineForm::Control(c) if v < Version::R2013 => {
                assert_eq!(c.points.len(), 4, "{name}");
                assert_eq!((s.flags1, s.knot_parameter), (0, SPLINE_KNOT_CUSTOM));
            }
            f => panic!("{name}: 434: {f:?}"),
        }

        // MLINE 38B: scale 1.5 (40), justification 1 (70), flags 1 (71; ODS:
        // "open (1), closed (3)"), 8 vertices (72), 2 lines (73), style 18
        // (340), the first vertex's first line with parameters 0.75 and 0
        // (41) and no fill parameters (75 = 0).
        let Entity::MLine(m) = entity(&d, 0x38B) else {
            panic!("{name}: 38B");
        };
        assert_eq!(
            (m.scale, m.justification, m.flags, m.lines_in_style),
            (1.5, 1, 1, 2),
            "{name}"
        );
        assert_eq!((m.vertices.len(), m.style), (8, Handle(0x18)), "{name}");
        let first = &m.vertices[0];
        assert_eq!(first.direction, [0.0, 1.0, 0.0], "{name}");
        assert_eq!(first.lines[0].segment_params, [0.75, 0.0], "{name}");
        assert!(first.lines[0].fill_params.is_empty(), "{name}");
        // MLINE 3A6: flags 5 (has vertices, start caps suppressed).
        let Entity::MLine(m) = entity(&d, 0x3A6) else {
            panic!("{name}: 3A6");
        };
        assert_eq!(m.flags, 5, "{name}");
        // MLINE 3A5: the second line of its first vertex has four segment
        // parameters (74 = 4).
        let Entity::MLine(m) = entity(&d, 0x3A5) else {
            panic!("{name}: 3A5");
        };
        assert_eq!(m.vertices[0].lines[1].segment_params.len(), 4, "{name}");
    }
    if seen == 0 {
        common::skip("acadsharp-sample files");
    }
}

/// The same drawing saved as R2013 (AC1027) and R2018 (AC1032): every
/// entity of these types with the same handle has the same value.
#[test]
fn corpus_r2013_and_r2018_polylines_agree() {
    let (Some(old), Some(new)) = (
        common::corpus_file_or_skip("acadsharp-sample-ac1027"),
        common::corpus_file_or_skip("acadsharp-sample-ac1032"),
    ) else {
        return;
    };
    let (a, b) = (Drawing::open(&old).unwrap(), Drawing::open(&new).unwrap());
    let mut compared: BTreeMap<String, usize> = BTreeMap::new();
    for (h, oa) in a.objects() {
        let t = oa.type_name().into_owned();
        if !is_task_type(&t) {
            continue;
        }
        let (Object::Entity { entity: ea, .. }, Some(Object::Entity { entity: eb, .. })) =
            (oa, b.get(h))
        else {
            panic!("{h} {t}");
        };
        assert_eq!(ea, eb, "{h} {t}");
        if let (Entity::Spline(sa), Entity::Spline(sb)) = (ea, eb) {
            // `AsRead` values compare equal whatever they hold.
            assert_eq!(sa.stored_scenario(), sb.stored_scenario(), "{h}");
        }
        *compared.entry(t).or_default() += 1;
    }
    let want: BTreeMap<String, usize> = [
        ("LWPOLYLINE", 23),
        ("MLINE", 3),
        ("POLYLINE_3D", 1),
        ("POLYLINE_PFACE", 1),
        ("SPLINE", 2),
        ("VERTEX_3D", 5),
        ("VERTEX_PFACE", 5),
        ("VERTEX_PFACE_FACE", 2),
    ]
    .into_iter()
    .map(|(t, n)| (t.to_owned(), n))
    .collect();
    assert_eq!(compared, want);
}
