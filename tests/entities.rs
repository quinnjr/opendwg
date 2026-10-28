//! Corpus checks of the basic, text and insert entities: every
//! entity of these types decodes typed, the counts match the manifest, R2018
//! entities re-encode bit for bit, and values that pin ODS deviations are
//! asserted on real data.

mod common;

use std::collections::BTreeMap;

use opendwg::object::{
    ATTRIB_MULTI_LINE, ATTRIB_SINGLE_LINE, DecodeCtx, Entity, EntityList, Insert, Object,
    decode_object, encode_object,
};
use opendwg::{Drawing, Handle, Version};

/// The entity types checked here, by type name, with their manifest count
/// keys.
const TYPES: [(&str, &str); 20] = [
    ("LINE", "line_count"),
    ("POINT", "point_count"),
    ("CIRCLE", "circle_count"),
    ("ARC", "arc_count"),
    ("ELLIPSE", "ellipse_count"),
    ("RAY", "ray_count"),
    ("XLINE", "xline_count"),
    ("SOLID", "solid_count"),
    ("TRACE", "trace_count"),
    ("3DFACE", "face3d_count"),
    ("SHAPE", "shape_count"),
    ("BLOCK", "block_count"),
    ("ENDBLK", "endblk_count"),
    ("SEQEND", "seqend_count"),
    ("INSERT", "insert_count"),
    ("MINSERT", "minsert_count"),
    ("TEXT", "text_count"),
    ("ATTRIB", "attrib_count"),
    ("ATTDEF", "attdef_count"),
    ("MTEXT", "mtext_count"),
];

fn is_task_type(name: &str) -> bool {
    TYPES.iter().any(|(t, _)| *t == name)
}

fn entities(d: &Drawing) -> impl Iterator<Item = (Handle, &Entity)> {
    d.objects().filter_map(|(h, o)| match o {
        Object::Entity { entity, .. } => Some((h, entity)),
        _ => None,
    })
}

#[test]
fn corpus_basic_entities_decode() {
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
            // SEQEND also ends polylines (tests/polylines.rs); those are counted too.
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
fn corpus_basic_entities_r2018_bytes() {
    let mut types = std::collections::BTreeSet::new();
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
    // TRACE and MINSERT are not in the corpus.
    if !types.is_empty() {
        assert_eq!(types.len(), 18, "{types:?}");
    }
}

fn inserts(d: &Drawing) -> BTreeMap<Handle, Insert> {
    entities(d)
        .filter_map(|(h, e)| match e {
            Entity::Insert(i) => Some((h, i.clone())),
            _ => None,
        })
        .collect()
}

/// R2000 inserts store their first and last attribute; the drawing turns
/// them into full lists, which must equal the R2004 file's stored lists (the
/// two samples are one drawing with the same handles).
#[test]
fn corpus_r2000_insert_attrib_lists() {
    let (Some(r2000), Some(r2004)) = (
        common::corpus_file_or_skip("acadsharp-sample-ac1015"),
        common::corpus_file_or_skip("acadsharp-sample-ac1018"),
    ) else {
        return;
    };
    let a = Drawing::open(&r2000).unwrap();
    let b = Drawing::open(&r2004).unwrap();
    assert!(a.diagnostics().is_empty(), "{:?}", a.diagnostics());
    let (ia, ib) = (inserts(&a), inserts(&b));
    assert_eq!(ia.len(), 14);
    // Two inserts of a renumbered anonymous block have other handles.
    let (mut same, mut with_attribs) = (0, 0);
    for (h, i) in &ia {
        let EntityList::Owned(list) = &i.attribs else {
            panic!("{h}: list not built");
        };
        // With the lists built, the R2000 inserts convert to R2018.
        encode_object(a.get(*h).unwrap(), &a.classes).unwrap();
        let Some(other) = ib.get(h) else {
            continue;
        };
        same += 1;
        assert_eq!(&i.attribs, &other.attribs, "{h}");
        assert_eq!(i.seqend, other.seqend, "{h}");
        if i.seqend.is_some() {
            with_attribs += 1;
            assert!(!list.is_empty(), "{h}");
        }
    }
    assert_eq!((same, with_attribs), (12, 2));
    // DXF: INSERT 79C owns ATTRIBs 79D, 79F and 7A0, ended by SEQEND 79E.
    let i = &ia[&Handle(0x79C)];
    assert_eq!(
        i.attribs,
        EntityList::Owned(vec![Handle(0x79D), Handle(0x79F), Handle(0x7A0)])
    );
    assert_eq!(i.seqend, Some(Handle(0x79E)));
}

/// Values of the sample drawing's text and insert entities, read from every
/// version, against its DXF exports (`sample_AC10xx_ascii.dxf`).
#[test]
fn corpus_text_values() {
    let mut seen = 0;
    for (e, path) in common::fetched_files_or_skip() {
        if !e.name.starts_with("acadsharp-sample-") {
            continue;
        }
        seen += 1;
        let name = &e.name;
        let d = Drawing::open(&path).unwrap();
        let v = d.source_version().unwrap();
        let get = |h: u64| match d.get(Handle(h)) {
            Some(Object::Entity { entity, .. }) => entity,
            o => panic!("{name}: {h:X}: {o:?}"),
        };

        // ATTDEF 6F8: tag ATTINFO, prompt "Enter number:", value 1, height 5;
        // no 11 group: the alignment point is not stored.
        let Entity::AttDef(def) = get(0x6F8) else {
            panic!("{name}: 6F8");
        };
        assert_eq!(def.attrib.tag, "ATTINFO", "{name}");
        assert_eq!(def.prompt, "Enter number:", "{name}");
        assert_eq!(def.attrib.text.value, "1", "{name}");
        assert_eq!(def.attrib.text.height, 5.0, "{name}");
        assert_eq!(def.attrib.text.alignment, None, "{name}");
        assert_eq!(def.attrib.attribute_type, ATTRIB_SINGLE_LINE, "{name}");

        // ATTRIB 79F: PRESET_ATT = "hello", 74 = 3, 280 (lock) = 1 from
        // R2007; the 11 group repeats the insertion X.
        let Entity::Attrib(a) = get(0x79F) else {
            panic!("{name}: 79F");
        };
        assert_eq!(
            (a.tag.as_str(), a.text.value.as_str()),
            ("PRESET_ATT", "hello")
        );
        assert_eq!(a.text.vertical_alignment, 3, "{name}");
        assert_eq!(a.lock_position, v >= Version::R2007, "{name}");
        let [x, y, z] = a.text.insertion;
        let align = a.text.alignment.unwrap_or_else(|| panic!("{name}: no 11"));
        assert_eq!(align[0], x, "{name}");
        assert!((align[1] - y - a.text.height).abs() < 1e-9, "{name}");
        assert_eq!(z, 0.0, "{name}");

        // ATTRIB 79D: multi-line from R2018, embedding an MTEXT (101
        // "Embedded Object") with the text, 71 = 1, 72 = 5.
        let Entity::Attrib(a) = get(0x79D) else {
            panic!("{name}: 79D");
        };
        if v >= Version::R2018 {
            assert_eq!(a.attribute_type, ATTRIB_MULTI_LINE, "{name}");
            assert_eq!(a.tag, "MULTI_LINE_ATT", "{name}");
            let m = a
                .mtext
                .as_ref()
                .unwrap_or_else(|| panic!("{name}: no MTEXT"));
            assert_eq!(m.mtext.contents, "my multi line text for the attrrib");
            assert_eq!((m.mtext.attachment, m.mtext.drawing_dir), (1, 5));
            assert_eq!(m.mtext.text_height, a.text.height, "{name}");
            assert_eq!(m.common.layer, Handle(0x10), "{name}");
            assert!(a.text.value.is_empty(), "{name}");
        } else {
            assert_eq!(a.attribute_type, ATTRIB_SINGLE_LINE, "{name}");
            assert!(a.mtext.is_none(), "{name}");
            assert_eq!(a.text.value, "my multi line text for the attrrib");
        }

        // MTEXT 3EC: two paragraphs.
        let Entity::MText(m) = get(0x3EC) else {
            panic!("{name}: 3EC");
        };
        assert_eq!(
            m.plain_text(),
            "this is a Mtext\nwith multiple lines in it",
            "{name}"
        );
        // Not annotative: R2018 stores a context, older files get one.
        assert!(m.context.is_some(), "{name}");

        // SHAPE 889: shape 1 of the shape file 887.
        let Entity::Shape(s) = get(0x889) else {
            panic!("{name}: 889");
        };
        assert_eq!((s.shape_number, s.style), (1, Handle(0x887)), "{name}");
    }
    if seen == 0 {
        common::skip("acadsharp-sample files");
    }
}

/// Common-data forms AutoCAD uses on entities, which R2018 output must
/// reproduce bit for bit (`corpus_basic_entities_r2018_bytes`), pinned by
/// value against the DXF exports.
#[test]
fn corpus_entity_common_forms() {
    let mut seen = 0;
    for (e, path) in common::fetched_files_or_skip() {
        if !e.name.starts_with("acadsharp-sample-") {
            continue;
        }
        seen += 1;
        let name = &e.name;
        let d = Drawing::open(&path).unwrap();
        let v = d.source_version().unwrap();
        let common = |h: u64| match d.get(Handle(h)) {
            Some(Object::Entity { common, .. }) => common.clone(),
            o => panic!("{name}: {h:X}: {o:?}"),
        };
        // Owners stored as offsets from the entity's handle (codes 6, 8, 0xA,
        // 0xC) or as soft pointers: 242 and 4 of the owned entities in every
        // version. BLOCK 5A's owner 58 is "handle - 2" (code 0xC).
        let (mut relative, mut absolute) = (0, 0);
        for (_, o) in d.objects() {
            // Not a let chain: the crate's rust-version (1.85) has none.
            if let Object::Entity { common, .. } = o {
                if common.entmode != 0 {
                    continue;
                }
                if common.obj.owner_relative {
                    relative += 1;
                } else {
                    absolute += 1;
                }
            }
        }
        assert_eq!((relative, absolute), (242, 4), "{name}");
        let block = common(0x5A);
        assert_eq!(
            (block.obj.owner, block.obj.owner_relative),
            (Handle(0x58), true)
        );

        // True colors keep the nearest ACI beside them (DXF 62 and 420):
        // POINT 29A is 230 and 0xF50A93, MTEXT 9DA 151 and 0x50B3FF. R2000
        // stores the ACI only.
        for (h, aci, (r, g, b)) in [(0x29A, 230, (245, 10, 147)), (0x9DA, 151, (80, 179, 255))] {
            let c = common(h);
            if v >= Version::R2004 {
                assert_eq!(
                    c.color,
                    opendwg::bits::Color::Rgb { r, g, b },
                    "{name}: {h:X}"
                );
                assert_eq!(c.true_color_aci(), aci, "{name}: {h:X}");
            } else {
                assert_eq!(c.color, opendwg::bits::Color::Index(aci as u8), "{name}");
                assert_eq!(c.true_color_aci(), 0, "{name}");
            }
        }

        // MTEXT 9DA: background fill (90 = 1) with scale 1.25 (45; a BD, not
        // ODS's BL), color 0x367542 (421) and transparency 0 (441).
        let mtext = |h: u64| match d.get(Handle(h)) {
            Some(Object::Entity {
                entity: Entity::MText(m),
                ..
            }) => m.clone(),
            o => panic!("{name}: {h:X}: {o:?}"),
        };
        let m = mtext(0x9DA);
        if v >= Version::R2004 {
            assert_eq!(m.background_flags, 1, "{name}");
            let bg = m.background.as_ref().unwrap();
            assert_eq!(bg.scale, 1.25, "{name}");
            assert_eq!(
                bg.color,
                opendwg::bits::Color::Rgb {
                    r: 0x36,
                    g: 0x75,
                    b: 0x42
                },
                "{name}"
            );
            assert_eq!(bg.transparency, 0, "{name}");
        } else {
            assert_eq!((m.background_flags, &m.background), (0, &None), "{name}");
        }
        // The R2018 context: version 4 (ODS: default 0), no registered
        // application, copies of the main fields; older files get the same.
        let c = m.context.as_ref().unwrap();
        assert_eq!(c, &m.context_from_fields(), "{name}");
        assert_eq!(c.version, opendwg::object::MTEXT_CONTEXT_VERSION);
        // MTEXT 779 has dynamic columns (R2018 embedded object: 71 = 2, 72 =
        // 1, 44 width, 45 gutter, 73 and 74 = 0; before R2018 the same in
        // ACAD extended data, moved into the context on read). The one
        // height is the negated extents height (DXF 43 = 204.3497...).
        let m = mtext(0x779);
        let cols = m.context.as_ref().unwrap().columns.as_ref();
        let cols = cols.unwrap_or_else(|| panic!("{name}: no columns"));
        assert_eq!((cols.column_type, cols.count), (2, 1), "{name}");
        assert!((cols.width - 15.74141424498748).abs() < 1e-12, "{name}");
        assert!((cols.gutter - 110.2231670381195).abs() < 1e-12, "{name}");
        assert!(!cols.auto_height && !cols.flow_reversed, "{name}");
        assert_eq!(cols.heights, [-m.extents_height], "{name}");
        assert!(
            (m.extents_height - 204.3497044110957).abs() < 1e-9,
            "{name}"
        );
        // The copies the R2018 context is written with equal the main fields.
        let copies = m.context_copies();
        assert_eq!(
            (copies.insertion, copies.rect_width, copies.extents_height),
            (m.insertion, m.rect_width, m.extents_height),
            "{name}"
        );

        // INSERT 660 has no attributes: from R2004 no owned-object count is
        // stored (ODS: always), or it would not decode to the last bit.
        let i = &inserts(&d)[&Handle(0x660)];
        assert_eq!((i.seqend, &i.attribs), (None, &EntityList::Owned(vec![])));
    }
    if seen == 0 {
        common::skip("acadsharp-sample files");
    }
}

/// The same drawing saved as R2013 (AC1027) and R2018 (AC1032): typed
/// entities with the same handle have the same values, so converting the
/// older file loses nothing R2018 stores, annotative state included (no
/// MTEXT of the R2018 file is annotative, and none read from R2013 is).
/// MTEXT columns, extended data before R2018, become the R2018 context's
/// columns and the extended data goes, as AutoCAD 2018 converts them. The
/// remaining differences are R2018-only: multi-line attributes (an
/// extension dictionary before R2018) and an insert of a renumbered
/// anonymous block.
#[test]
fn corpus_r2013_and_r2018_entities_agree() {
    let (Some(old), Some(new)) = (
        common::corpus_file_or_skip("acadsharp-sample-ac1027"),
        common::corpus_file_or_skip("acadsharp-sample-ac1032"),
    ) else {
        return;
    };
    let (a, b) = (Drawing::open(&old).unwrap(), Drawing::open(&new).unwrap());
    let mut compared: BTreeMap<String, usize> = BTreeMap::new();
    let mut differ = vec![];
    for (h, oa) in a.objects() {
        let t = oa.type_name().into_owned();
        let (
            Object::Entity {
                entity: ea,
                common: ca,
            },
            Some(Object::Entity {
                entity: eb,
                common: cb,
            }),
        ) = (oa, b.get(h))
        else {
            continue;
        };
        if !is_task_type(&t) {
            continue;
        }
        *compared.entry(t.clone()).or_default() += 1;
        if ea != eb {
            differ.push(format!("{h} {t}"));
        }
        // `AsRead` values compare equal whatever they hold, so what is
        // written from them is compared explicitly.
        assert_eq!(ca.color, cb.color, "{h}");
        assert_eq!(ca.true_color_aci(), cb.true_color_aci(), "{h}");
        if let (Entity::MText(ma), Entity::MText(mb)) = (ea, eb) {
            assert_eq!(ca.obj.xdata, cb.obj.xdata, "{h}");
            assert!(ma.context.is_some(), "{h}");
            assert_eq!(ma.context_copies(), mb.context_copies(), "{h}");
            *compared.entry("MTEXT copies".into()).or_default() += 1;
        }
    }
    for (t, n) in [
        ("MTEXT", 35),
        ("ATTRIB", 4),
        ("ATTDEF", 5),
        ("LINE", 98),
        ("TEXT", 29),
        ("INSERT", 12),
    ] {
        assert_eq!(compared.get(t), Some(&n), "{t}");
    }
    assert_eq!(
        differ,
        ["3F5 MTEXT", "796 ATTDEF", "79D ATTRIB", "A44 INSERT"]
    );
    // The four MTEXTs with columns: dynamic, one column. R2013 stores the
    // height as 0; R2018 stores the negated extents height for three of
    // them, which the conversion reproduces, but the negated extents
    // *width* for 3F5, which nothing in the R2013 file gives. 3F5 differs
    // in that height only.
    let mtext = |d: &Drawing, h: u64| match d.get(Handle(h)) {
        Some(Object::Entity {
            entity: Entity::MText(m),
            ..
        }) => m.clone(),
        o => panic!("{h:X}: {o:?}"),
    };
    for h in [0x3EC, 0x3F5, 0x3F6, 0x779] {
        let (ma, mb) = (mtext(&a, h), mtext(&b, h));
        let ca = ma.context.as_ref().unwrap().columns.clone().unwrap();
        let cb = mb.context.as_ref().unwrap().columns.clone().unwrap();
        assert_eq!((ca.column_type, ca.count), (2, 1), "{h:X}");
        assert_eq!(ca.heights, [-ma.extents_height], "{h:X}");
        let want = if h == 0x3F5 {
            -mb.extents_width
        } else {
            -mb.extents_height
        };
        assert_eq!(cb.heights, [want], "{h:X}");
        let mut fixed = mb.clone();
        fixed.context.as_mut().unwrap().columns = Some(ca);
        assert_eq!(fixed, ma, "{h:X}");
    }
    let Some(Object::Entity {
        entity: Entity::Insert(ia),
        ..
    }) = a.get(Handle(0xA44))
    else {
        panic!("A44");
    };
    assert_ne!(ia.block, Handle(0));
}

/// The ACI index AutoCAD stores beside each true color of the corpus, and
/// the one `opendwg::bits::nearest_aci` (smallest squared distance in the
/// standard palette) gives when the color is edited. They agree on 3 of the
/// 6 colors, with the structurally checked palette as with ezdxf's,
/// netDxf's or ACadSharp's tables; AutoCAD's own choice follows no metric
/// tried (squared, as netDxf uses, Manhattan, Chebyshev, luma-weighted,
/// "redmean", HSV and CIE Lab distances all miss some). Unedited colors keep AutoCAD's index, so the
/// R2018 output stays bit-identical; only an edited color gets the nearest
/// index.
#[test]
fn corpus_true_color_aci_values() {
    // (r, g, b), AutoCAD's index, nearest_aci.
    let known = [
        ((44, 136, 27), 86, 84),
        ((52, 201, 24), 82, 82),
        ((80, 179, 255), 151, 151),
        ((128, 61, 194), 183, 193),
        ((155, 66, 236), 193, 181),
        ((245, 10, 147), 230, 230),
    ];
    let mut seen = std::collections::BTreeSet::new();
    for (e, path) in common::fetched_files_or_skip() {
        let d = Drawing::open(&path).unwrap();
        for (h, _) in entities(&d) {
            let Some(Object::Entity { common, .. }) = d.get(h) else {
                continue;
            };
            let opendwg::bits::Color::Rgb { r, g, b } = common.color else {
                continue;
            };
            let (_, autocad, nearest) = known
                .iter()
                .find(|(rgb, ..)| *rgb == (r, g, b))
                .unwrap_or_else(|| panic!("{}: {h}: new color ({r}, {g}, {b})", e.name));
            assert_eq!(common.true_color_aci(), *autocad, "{}: {h}", e.name);
            assert_eq!(opendwg::bits::nearest_aci(r, g, b), *nearest);
            // Edited (here: set again by hand, nothing read): the nearest.
            let mut edited = common.clone();
            edited.read_color = Default::default();
            assert_eq!(edited.true_color_aci(), u16::from(*nearest));
            seen.insert((r, g, b));
        }
    }
    if seen.is_empty() {
        common::skip("true-color entities");
    } else {
        assert_eq!(seen.len(), known.len());
    }
}
