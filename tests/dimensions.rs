//! Corpus checks of the dimensions, leaders, tolerances and multileaders:
//! every entity of these types decodes typed, the counts match
//! the manifest, R2018 entities re-encode bit for bit, older versions
//! convert, and values that pin ODS deviations are asserted on real data.
//!
//! ARC_DIMENSION and LARGE_RADIAL_DIMENSION are not in the corpus; their
//! unit tests in `src/object/entities/dim.rs` are hermetic.

mod common;

use std::collections::{BTreeMap, BTreeSet};

use opendwg::bits::Color;
use opendwg::object::{
    DecodeCtx, DimKind, Dimension, Entity, MLeaderContent, NonGraphical, Object, decode_object,
    encode_object,
};
use opendwg::{Drawing, Handle, Version};

/// The entity types checked here, by type name, with their manifest count
/// keys.
const TYPES: [(&str, &str); 12] = [
    ("DIMENSION_ORDINATE", "dimension_ordinate_count"),
    ("DIMENSION_LINEAR", "dimension_linear_count"),
    ("DIMENSION_ALIGNED", "dimension_aligned_count"),
    ("DIMENSION_ANG3PT", "dimension_ang3pt_count"),
    ("DIMENSION_ANG2LN", "dimension_ang2ln_count"),
    ("DIMENSION_RADIUS", "dimension_radius_count"),
    ("DIMENSION_DIAMETER", "dimension_diameter_count"),
    ("ARC_DIMENSION", "arc_dimension_count"),
    ("LARGE_RADIAL_DIMENSION", "large_radial_dimension_count"),
    ("LEADER", "leader_count"),
    ("TOLERANCE", "tolerance_count"),
    ("MULTILEADER", "multileader_count"),
];

/// The types the corpus has.
const IN_CORPUS: usize = 10;

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
fn corpus_dimensions_decode() {
    for (e, path) in common::fetched_files_or_skip() {
        let name = &e.name;
        let d = Drawing::open(&path).unwrap();
        let fallbacks: Vec<_> = d
            .diagnostics()
            .iter()
            .filter(|x| x.message.contains("kept raw"))
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
fn corpus_dimensions_r2018_bytes() {
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
    if !types.is_empty() {
        assert_eq!(types.len(), IN_CORPUS, "{types:?}");
    }
}

/// Every entity of these types read from an older version encodes as R2018
/// and decodes back to the same value.
#[test]
fn corpus_dimensions_older_versions_convert() {
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
            if let Object::Entity { common, entity } = &mut want {
                common.prev_next = None;
                // R2010+ leaders store no text box size.
                if let Entity::Leader(l) = entity {
                    l.box_size = None;
                }
            }
            assert_eq!(back, want, "{}: {h} {t}", e.name);
            // The measurement written is the one read.
            if let (
                Object::Entity {
                    entity: Entity::Dimension(a),
                    ..
                },
                Object::Entity {
                    entity: Entity::Dimension(b),
                    ..
                },
            ) = (obj, &back)
            {
                assert_eq!(
                    a.measurement().to_bits(),
                    b.common.measurement.to_bits(),
                    "{}: {h}",
                    e.name
                );
            }
            seen.entry(v).or_default().insert(t);
        }
    }
    for (v, types) in seen {
        assert_eq!(types.len(), IN_CORPUS, "{v:?}: {types:?}");
    }
}

fn dimension(d: &Drawing, h: u64) -> &Dimension {
    match entity(d, h) {
        Entity::Dimension(x) => x,
        e => panic!("{h:X}: {e:?}"),
    }
}

/// The name of the table entry `h` (a block record or a dimension style).
fn entry_name(d: &Drawing, h: Handle) -> &str {
    match d.get(h) {
        Some(Object::Other { object, .. }) => &object.table_entry().unwrap().name,
        o => panic!("{h}: {o:?}"),
    }
}

/// `a` and `b` agree to about 12 significant digits.
fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-12 * (1.0 + a.abs().max(b.abs()))
}

/// Values of the sample drawing's dimensions, leaders, tolerances and
/// multileaders, read from every version, against its DXF exports
/// (`sample_AC10xx_ascii.dxf`).
#[test]
fn corpus_dimension_values() {
    let mut seen = 0;
    for (e, path) in common::fetched_files_or_skip() {
        if !e.name.starts_with("acadsharp-sample-") {
            continue;
        }
        seen += 1;
        let name = &e.name;
        let d = Drawing::open(&path).unwrap();
        let v = d.source_version().unwrap();

        // DIMENSION 514 (DXF 70 = 32: rotated, block used by this dimension
        // only): style ISO-25, block *D3 (stored as "*D": a DWG keeps
        // anonymous block names unnumbered, the DXF export numbers them), rotation 50 = 30 degrees, actual
        // measurement 42 = 46.7156... "Flags 1" is 11: bit 0 (text at its
        // default position: DXF 70 lacks 128), bit 1 (DXF 70 has 32) and
        // bit 3, which ODS does not describe.
        let x = dimension(&d, 0x514);
        let DimKind::Linear {
            ext1,
            ext2,
            rotation,
            ..
        } = x.kind
        else {
            panic!("{name}: 514 {:?}", x.kind);
        };
        assert!(close(ext1[0], 330.2890594765795) && close(ext1[1], 2.941179455067986));
        assert!(close(ext2[0], 364.4872644138525) && close(ext2[1], 37.13938439234094));
        assert!(
            close(rotation.to_degrees(), 30.00000000000059),
            "{name}: {rotation}"
        );
        assert_eq!(x.common.measurement, 46.71561670814134, "{name}");
        assert_eq!(x.common.flags, 11, "{name}");
        assert_eq!(x.common.attachment, 5, "{name}");
        assert_eq!(entry_name(&d, x.common.dimstyle), "ISO-25", "{name}");
        assert_eq!(entry_name(&d, x.common.block), "*D", "{name}");
        // DXF 280: the R2010+ class version is 0 (and 0 is the default
        // before R2010).
        assert_eq!(x.common.class_version, 0, "{name}");
        // DIMENSION 522 (diameter, DXF 70 = 163: 128 set, so bit 0 clear).
        let x = dimension(&d, 0x522);
        assert_eq!(x.common.flags, 10, "{name}");
        assert!(matches!(x.kind, DimKind::Diameter { .. }), "{name}");
        assert_eq!(entry_name(&d, x.common.block), "*D", "{name}");
        // DIMENSION 526 is an X ordinate (DXF 70 = 102 has 64), 525 a Y
        // ordinate (38).
        for (h, flags2) in [(0x526, 1), (0x525, 0)] {
            let DimKind::Ordinate { flags2: f, .. } = dimension(&d, h).kind else {
                panic!("{name}: {h:X}");
            };
            assert_eq!(f, flags2, "{name}: {h:X}");
        }
        // DIMENSION 9CE: style custom_dim_style, block *D14, measurement
        // 7.0710678118654755 (5 * sqrt 2).
        let x = dimension(&d, 0x9CE);
        assert_eq!(entry_name(&d, x.common.dimstyle), "custom_dim_style");
        assert_eq!(x.common.measurement, 7.0710678118654755, "{name}");

        // Every dimension: the measurement written is the one read, and the
        // formula for its kind gives the stored value (within 1e-9).
        let mut computed = 0;
        for (h, obj) in d.objects() {
            let Object::Entity {
                entity: Entity::Dimension(x),
                ..
            } = obj
            else {
                continue;
            };
            assert_eq!(
                x.measurement().to_bits(),
                x.common.measurement.to_bits(),
                "{name}: {h}"
            );
            // Ordinates too: 525 (Y) is |8.6409 - 111.2355| = 102.5946 and
            // 526 (X) |666.5714 - 313.1900| = 353.3815, along the UCS axes
            // given by DXF 51 (0 here).
            let c = &x.common;
            let m = x
                .kind
                .computed_measurement(c.extrusion, c.horiz_dir)
                .unwrap_or_else(|| panic!("{name}: {h}: no formula"));
            let stored = c.measurement;
            assert!(
                (m - stored).abs() <= 1e-9 * stored,
                "{name}: {h}: {m} vs {stored}"
            );
            computed += 1;
        }
        assert_eq!(computed, 11, "{name}");

        // LEADER 512: style ISO-25, annotation 513 (340), created with text
        // (73 = 0). The DXF export lists four points (76) as the leader has
        // a hook line (75 = 1): the file stores three, and the export adds
        // the hook line's start before the last. The text box (40, 41) is
        // stored up to R2007 only.
        let Entity::Leader(l) = entity(&d, 0x512) else {
            panic!("{name}: 512");
        };
        assert_eq!(l.annotation, Handle(0x513), "{name}");
        assert_eq!(entry_name(&d, l.dimstyle), "ISO-25", "{name}");
        assert_eq!((l.annot_type, l.path_type, l.points.len()), (0, 0, 3));
        assert!(close(l.points[0][0], 717.8687154323397), "{name}");
        assert!(close(l.points[1][1], 48.53878603809858), "{name}");
        assert!(close(l.points[2][0], 740.3987500252849), "{name}");
        assert!(l.arrowhead_on && !l.hook_line_on_x_dir, "{name}");
        if v <= Version::R2007 {
            let [h, w] = l.box_size.unwrap();
            assert!(
                close(h, 1.844821309997272) && close(w, 16.37400410337938),
                "{name}"
            );
        } else {
            assert_eq!(l.box_size, None, "{name}");
        }

        // TOLERANCE 823: its text is a TV (ODS: BS), "99^Ja" in DXF.
        let Entity::Tolerance(t) = entity(&d, 0x823) else {
            panic!("{name}: 823");
        };
        assert_eq!(t.text, "99\na", "{name}");
        assert_eq!(entry_name(&d, t.dimstyle), "Standard", "{name}");
        assert!(close(t.insertion[0], 215.9040473487617), "{name}");
        let Entity::Tolerance(t) = entity(&d, 0x824) else {
            panic!("{name}: 824");
        };
        assert_eq!(t.text, "{\\Fgdt;p}", "{name}");

        // MULTILEADER 7B8: style D8 (340), overrides 279552 (90), linetype
        // 14 (341), text style 11 (343), text alignment 175 = 0 (ODS:
        // unknown) and 179 = 1, landing 0.36 (41); its context: text height
        // 0.18 (41), flow direction 5 (172), one root with landing 0.36 and
        // one line of one point.
        let Entity::MultiLeader(m) = entity(&d, 0x7B8) else {
            panic!("{name}: 7B8");
        };
        assert_eq!(
            (m.style, m.line_type, m.text_style),
            (Handle(0xD8), Handle(0x14), Handle(0x11))
        );
        assert_eq!(m.override_flags, 279552, "{name}");
        assert_eq!(
            (m.text_alignment, m.justification, m.content_type),
            (0, 1, 2)
        );
        assert_eq!(
            (m.landing_distance, m.arrowhead_size),
            (0.36, 0.18),
            "{name}"
        );
        assert_eq!(
            (m.line_weight, m.line_color.clone()),
            (-2, Color::ByBlock),
            "{name}"
        );
        assert_eq!(m.version, 2, "{name}");
        assert!(
            m.arrowheads.is_empty() && m.block_labels.is_empty(),
            "{name}"
        );
        let c = &m.context;
        assert_eq!((c.text_height, c.landing_gap), (0.18, 0.09), "{name}");
        let MLeaderContent::Text(text) = &c.content else {
            panic!("{name}: 7B8 {:?}", c.content);
        };
        assert_eq!(text.label, "MLeader text, hello!", "{name}");
        assert_eq!(
            (text.style, text.flow_direction, text.alignment),
            (Handle(0x11), 5, 1)
        );
        assert!(close(text.boundary_width, 1.447121418826739), "{name}");
        assert_eq!(c.roots.len(), 1, "{name}");
        let root = &c.roots[0];
        assert_eq!(
            (root.landing_distance, root.lines.len()),
            (0.36, 1),
            "{name}"
        );
        assert!(
            close(root.lines[0].points[0][0], 1050.241562773277),
            "{name}"
        );
        assert!(close(root.connection[0], 1083.321045165764), "{name}");
        // MULTILEADER B27 attaches vertically (271 = 1), which R2010 added:
        // up to R2007 it reads as horizontal with AutoCAD's defaults.
        let Entity::MultiLeader(m) = entity(&d, 0xB27) else {
            panic!("{name}: B27");
        };
        let c = &m.context;
        if v >= Version::R2010 {
            assert_eq!(
                (m.attachment_direction, c.roots[0].attachment_direction),
                (1, 1)
            );
            assert_eq!(
                (c.text_left_attachment, c.text_right_attachment),
                (0, 8),
                "{name}"
            );
            assert_eq!(m.override_flags, 33838080, "{name}");
        } else {
            assert_eq!(
                (m.attachment_direction, c.roots[0].attachment_direction),
                (0, 0)
            );
        }
        assert_eq!(
            (m.text_top_attachment, m.text_bottom_attachment),
            (9, 9),
            "{name}"
        );
        assert_eq!(
            (c.text_top_attachment, c.text_bottom_attachment),
            (9, 9),
            "{name}"
        );
    }
    if seen == 0 {
        common::skip("acadsharp-sample files");
    }
}

/// The same drawing saved as R2013 (AC1027) and R2018 (AC1032): every
/// entity of these types with the same handle has the same value, and the
/// same measurement is written for each dimension.
#[test]
fn corpus_r2013_and_r2018_dimensions_agree() {
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
        if let (Entity::Dimension(da), Entity::Dimension(db)) = (ea, eb) {
            // The derived copy: stored and written values, bit for bit.
            assert_eq!(
                da.common.measurement.to_bits(),
                db.common.measurement.to_bits()
            );
            assert_eq!(
                da.measurement().to_bits(),
                db.measurement().to_bits(),
                "{h}"
            );
        }
        *compared.entry(t).or_default() += 1;
    }
    let want: BTreeMap<String, usize> = [
        ("DIMENSION_ALIGNED", 2),
        ("DIMENSION_ANG2LN", 1),
        ("DIMENSION_ANG3PT", 1),
        ("DIMENSION_DIAMETER", 1),
        ("DIMENSION_LINEAR", 3),
        ("DIMENSION_ORDINATE", 2),
        ("DIMENSION_RADIUS", 1),
        ("LEADER", 1),
        ("MULTILEADER", 15),
        ("TOLERANCE", 3),
    ]
    .into_iter()
    .map(|(t, n)| (t.to_owned(), n))
    .collect();
    assert_eq!(compared, want);
}

/// An edit to a corpus dimension's geometry rewrites its measurement from
/// the geometry; undoing the edit writes the stored value again, and the
/// entity re-encodes to its original bytes.
#[test]
fn corpus_dimension_edits_refresh_the_measurement() {
    let Some(path) = common::corpus_file_or_skip("acadsharp-sample-ac1032") else {
        return;
    };
    let bytes = std::fs::read(&path).unwrap();
    let d = Drawing::from_bytes(&bytes).unwrap();
    let originals = common::object_bytes(&bytes);
    let obj = d.get(Handle(0x527)).unwrap();
    let Object::Entity {
        common,
        entity: Entity::Dimension(x),
    } = obj
    else {
        panic!("{obj:?}");
    };
    let mut edited = (**x).clone();
    let DimKind::Aligned { ext2, .. } = &mut edited.kind else {
        panic!("{:?}", edited.kind);
    };
    let original = *ext2;
    // Ext 2 moved to 10 units right of ext 1.
    let DimKind::Aligned { ext1, .. } = x.kind else {
        unreachable!()
    };
    *ext2 = [ext1[0] + 10.0, ext1[1], ext1[2]];
    assert!(
        close(edited.measurement(), 10.0),
        "{}",
        edited.measurement()
    );
    let obj2 = Object::Entity {
        common: common.clone(),
        entity: Entity::Dimension(Box::new(edited.clone())),
    };
    let out = encode_object(&obj2, &d.classes).unwrap();
    let mut ctx = DecodeCtx::new(Version::R2018, 30, &d.classes);
    let Object::Entity {
        entity: Entity::Dimension(back),
        ..
    } = decode_object(&out, Handle(0x527), &mut ctx).unwrap()
    else {
        panic!();
    };
    assert!(close(back.common.measurement, 10.0));
    // Undone: the stored value and the original bytes.
    let DimKind::Aligned { ext2, .. } = &mut edited.kind else {
        unreachable!()
    };
    *ext2 = original;
    assert_eq!(edited.measurement(), 48.36356523110595);
    let obj3 = Object::Entity {
        common: common.clone(),
        entity: Entity::Dimension(Box::new(edited)),
    };
    assert_eq!(
        encode_object(&obj3, &d.classes).unwrap(),
        originals[&Handle(0x527)]
    );
    // The anonymous block stays as read.
    assert!(matches!(
        d.get(x.common.block),
        Some(Object::Other {
            object: NonGraphical::BlockRecord(_),
            ..
        })
    ));
}
