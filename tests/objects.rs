//! Corpus checks of the non-graphical objects of Task 18 (dictionaries,
//! extended records, layouts, styles, image definitions, scales, sort tables,
//! spatial filters and indexes, ...): every object of these types decodes
//! typed, the counts match the manifest, R2018 objects re-encode bit for bit,
//! older versions convert (or are refused when R2018 cannot hold them), and
//! values that pin ODS deviations are asserted on real data.
//!
//! PLOTSETTINGS, ACAD_PROXY_OBJECT and LAYER_INDEX are not in the corpus;
//! their unit tests in `src/object/objects.rs` are hermetic.

mod common;

use std::collections::{BTreeMap, BTreeSet};

use opendwg::bits::Color;
use opendwg::object::{
    DecodeCtx, Dictionary, MLineLinetype, NonGraphical, Object, TableStyle, XRecordValue,
    decode_object, encode_object,
};
use opendwg::{Drawing, Handle, Version};

/// The Task 18 object types, by type name, with their manifest count keys.
const TYPES: [(&str, &str); 21] = [
    ("DICTIONARY", "dictionary_count"),
    ("ACDBDICTIONARYWDFLT", "dictionary_with_default_count"),
    ("XRECORD", "xrecord_count"),
    ("LAYOUT", "layout_count"),
    ("PLOTSETTINGS", "plotsettings_count"),
    ("GROUP", "group_count"),
    ("MLINESTYLE", "mlinestyle_count"),
    ("MLEADERSTYLE", "mleaderstyle_count"),
    ("TABLESTYLE", "tablestyle_count"),
    ("IMAGEDEF", "imagedef_count"),
    ("IMAGEDEF_REACTOR", "imagedef_reactor_count"),
    ("RASTERVARIABLES", "rastervariables_count"),
    ("WIPEOUTVARIABLES", "wipeoutvariables_count"),
    ("SCALE", "scale_count"),
    ("DICTIONARYVAR", "dictionaryvar_count"),
    ("ACDBPLACEHOLDER", "placeholder_count"),
    ("SORTENTSTABLE", "sortentstable_count"),
    ("ACAD_PROXY_OBJECT", "proxy_object_count"),
    ("SPATIAL_INDEX", "spatial_index_count"),
    ("LAYER_INDEX", "layer_index_count"),
    ("SPATIAL_FILTER", "spatial_filter_count"),
];

/// The types the R2018 corpus files have.
const IN_R2018_CORPUS: usize = 18;

fn is_task_type(name: &str) -> bool {
    TYPES.iter().any(|(t, _)| *t == name)
}

fn r2018_ctx(d: &Drawing) -> DecodeCtx<'_> {
    DecodeCtx::new(Version::R2018, 30, &d.classes)
}

fn object(d: &Drawing, h: u64) -> &NonGraphical {
    match d.get(Handle(h)) {
        Some(Object::Other { object, .. }) => object,
        o => panic!("{h:X}: {o:?}"),
    }
}

#[test]
fn corpus_objects_decode() {
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
fn corpus_objects_r2018_bytes() {
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
            let back = decode_object(&out, h, &mut r2018_ctx(&d)).unwrap();
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
        assert_eq!(types.len(), IN_R2018_CORPUS, "{types:?}");
    }
}

/// The R2018 form of an object: a pre-R2010 table style is written as its
/// R2010 conversion.
fn r2018_form(obj: &Object) -> Object {
    let mut want = obj.clone();
    if let Object::Other {
        object: o @ NonGraphical::TableStyle(TableStyle::Legacy(_)),
        ..
    } = &mut want
    {
        let NonGraphical::TableStyle(TableStyle::Legacy(l)) = &*o else {
            unreachable!()
        };
        *o = NonGraphical::TableStyle(TableStyle::Cells(Box::new(l.to_r2010().unwrap())));
    }
    want
}

/// Every object of these types read from an older version encodes as R2018
/// and decodes back to the same value (a pre-R2010 table style to its R2010
/// conversion).
#[test]
fn corpus_objects_older_versions_convert() {
    let mut seen: BTreeMap<Version, BTreeSet<String>> = BTreeMap::new();
    let mut legacy: BTreeMap<Version, usize> = BTreeMap::new();
    for (e, path) in common::fetched_files_or_skip() {
        let d = Drawing::open(&path).unwrap();
        let v = d.source_version().unwrap();
        for (h, obj) in d.objects() {
            let t = obj.type_name().into_owned();
            if !is_task_type(&t) {
                continue;
            }
            seen.entry(v).or_default().insert(t.clone());
            if let Object::Other {
                object: NonGraphical::TableStyle(TableStyle::Legacy(_)),
                ..
            } = obj
            {
                assert!(v < Version::R2010, "{}: {h}", e.name);
                *legacy.entry(v).or_default() += 1;
            }
            let out = encode_object(obj, &d.classes)
                .unwrap_or_else(|err| panic!("{}: {h} {t}: {err}", e.name));
            let mut ctx = r2018_ctx(&d);
            let back = decode_object(&out, h, &mut ctx).unwrap();
            assert!(ctx.diagnostics.is_empty(), "{}: {h}", e.name);
            assert_eq!(back, r2018_form(obj), "{}: {h} {t}", e.name);
        }
    }
    for (v, types) in &seen {
        // R2000-R2013 samples: 17 types (no SPATIAL_INDEX).
        assert!(types.len() >= 17, "{v:?}: {types:?}");
    }
    // The one table style of each R2000-R2007 sample.
    for (v, n) in legacy {
        assert!(v < Version::R2010, "{v:?}");
        assert_eq!(n, 1, "{v:?}");
    }
}

/// The table style of the R2000, R2004 and R2007 samples converts to exactly
/// the R2010 form AutoCAD wrote for the same drawing in the R2010 sample.
#[test]
fn corpus_legacy_table_styles_convert_as_autocad_does() {
    let Some(r2010) = common::corpus_file_or_skip("acadsharp-sample-ac1024") else {
        return;
    };
    let r2010 = Drawing::open(r2010).unwrap();
    let NonGraphical::TableStyle(TableStyle::Cells(want)) = object(&r2010, 0x87) else {
        panic!("R2010 table style");
    };
    let mut n = 0;
    for v in ["ac1015", "ac1018", "ac1021"] {
        let Some(path) = common::corpus_file_or_skip(&format!("acadsharp-sample-{v}")) else {
            continue;
        };
        let d = Drawing::open(path).unwrap();
        let NonGraphical::TableStyle(TableStyle::Legacy(l)) = object(&d, 0x87) else {
            panic!("{v}");
        };
        assert_eq!(&l.to_r2010().unwrap(), want.as_ref(), "{v}");
        n += 1;
    }
    assert!(n == 0 || n == 3, "{n}");
}

/// The R2013 and R2018 samples hold the same drawing: their objects of these
/// types decode equal.
#[test]
fn corpus_r2013_and_r2018_objects_agree() {
    let (Some(a), Some(b)) = (
        common::corpus_file_or_skip("acadsharp-sample-ac1027"),
        common::corpus_file_or_skip("acadsharp-sample-ac1032"),
    ) else {
        return;
    };
    let a = Drawing::open(a).unwrap();
    let b = Drawing::open(b).unwrap();
    let mut n = 0;
    for (h, x) in a.objects() {
        if !is_task_type(&x.type_name()) {
            continue;
        }
        let Some(y) = b.get(h) else {
            continue;
        };
        let (Object::Other { object: x, .. }, Object::Other { object: y, .. }) = (x, y) else {
            panic!("{h}: not objects");
        };
        // Saved per file: the named object dictionary (the R2013 file has an
        // AcDsDecomposeData entry) and the last-saved version record.
        if [Handle(0xC), Handle(0xD09)].contains(&h) {
            assert_ne!(x, y, "{h}");
            continue;
        }
        assert_eq!(x, y, "{h}");
        n += 1;
    }
    assert!(n > 200, "{n}");
}

fn dictionary(d: &Drawing, h: u64) -> &Dictionary {
    match object(d, h) {
        NonGraphical::Dictionary(x) => x,
        o => panic!("{h:X}: {o:?}"),
    }
}

/// The named object dictionary and the multiline style of every sample
/// version, against the DXF exports.
#[test]
fn corpus_dictionary_and_style_values() {
    let mut seen = 0;
    for (e, path) in common::fetched_files_or_skip() {
        if !e.name.starts_with("acadsharp-sample-") {
            continue;
        }
        seen += 1;
        let name = &e.name;
        let d = Drawing::open(&path).unwrap();
        // The named object dictionary C: no hard owner flag (the DXF export
        // has no 280), entries in file order.
        let nod = dictionary(&d, 0xC);
        assert!(nod.hard_owner == 0 && nod.cloning == 1, "{name}");
        assert_eq!(nod.get("ACAD_GROUP"), Some(Handle(0xD)), "{name}");
        assert_eq!(nod.get("ACAD_MLINESTYLE"), Some(Handle(0x17)), "{name}");
        assert_eq!(nod.get("ACAD_LAYOUT"), Some(Handle(0x1A)), "{name}");
        // MLINESTYLE 18 "STANDARD": two lines at +-0.5, linetype BYLAYER
        // (15). Before R2018 the linetype is the index 32767, resolved on
        // read.
        let NonGraphical::MLineStyle(m) = object(&d, 0x18) else {
            panic!("{name}");
        };
        assert_eq!(m.name, "STANDARD", "{name}");
        assert_eq!(m.elements.len(), 2, "{name}");
        for (el, offset) in m.elements.iter().zip([0.5, -0.5]) {
            assert_eq!(el.offset, offset, "{name}");
            assert_eq!(el.color, Color::ByLayer, "{name}");
            assert_eq!(el.linetype, MLineLinetype::Handle(Handle(0x15)), "{name}");
        }
    }
    assert!(seen == 0 || seen == 6, "{seen}");
}

/// Extended record values: every item of every record reads as its group
/// code's type, and the last-saved record D09 of each sample (code page text
/// before R2007, UTF-16 after) names its own file.
#[test]
fn corpus_xrecord_values() {
    for (e, path) in common::fetched_files_or_skip() {
        let d = Drawing::open(&path).unwrap();
        if let Some(suffix) = e.name.strip_prefix("acadsharp-sample-") {
            let NonGraphical::XRecord(x) = object(&d, 0xD09) else {
                panic!("{}", e.name);
            };
            let codes: Vec<i16> = x.items.iter().map(|(c, _)| *c).collect();
            assert_eq!(codes, [300, 301, 302, 303], "{}", e.name);
            let XRecordValue::Text(path) = &x.items[3].1 else {
                panic!("{}", e.name);
            };
            let want = format!("\\sample_{}.dwg", suffix.to_uppercase());
            assert!(path.ends_with(&want), "{}: {path}", e.name);
            assert!(path.starts_with("C:\\Users\\"), "{}: {path}", e.name);
            assert_eq!(x.cloning, 1, "{}", e.name);
        }
        for (h, obj) in d.objects() {
            let Object::Other {
                object: NonGraphical::XRecord(x),
                ..
            } = obj
            else {
                continue;
            };
            for (code, v) in &x.items {
                assert!(
                    !matches!(v, XRecordValue::Unknown(_)),
                    "{}: {h}: code {code}: {v:?}",
                    e.name
                );
            }
        }
    }
}

/// Values of the sample drawing's objects, read from every version, against
/// its DXF exports (`sample_AC10xx_ascii.dxf`). They pin where ODS and the
/// files differ.
#[test]
fn corpus_object_values() {
    let mut filter = None;
    let mut seen = 0;
    for (e, path) in common::fetched_files_or_skip() {
        if !e.name.starts_with("acadsharp-sample-") {
            continue;
        }
        seen += 1;
        let name = &e.name;
        let d = Drawing::open(&path).unwrap();
        let v = d.source_version().unwrap();
        // DICTIONARY 18E: hard owner (DXF 280 = 1, entries as 360), yet its
        // entries are soft owners like every dictionary's (ODS: soft owner;
        // the R2018 re-encode test checks the stored code).
        let x = dictionary(&d, 0x18E);
        assert_eq!(x.hard_owner, 1, "{name}");
        assert_eq!(
            x.entries[0],
            ("ACAD_LAYERSTATES".into(), Handle(0x18F)),
            "{name}"
        );
        // ACDBDICTIONARYWDFLT E: Normal -> F, the default.
        let NonGraphical::DictionaryWithDefault(x) = object(&d, 0xE) else {
            panic!("{name}");
        };
        assert_eq!(x.dictionary.entries, [("Normal".into(), Handle(0xF))]);
        assert_eq!(x.default_entry, Handle(0xF), "{name}");
        assert!(matches!(object(&d, 0xF), NonGraphical::Placeholder));
        // LAYOUT 59 "Layout1". Its viewport list (stored as a BL count, ODS:
        // RL) is the VIEWPORT entities of its block; R2000 stores none and
        // gets them from the block. R2000 has no shade plot fields: the
        // R2004 values 0, 2, 300.
        let NonGraphical::Layout(l) = object(&d, 0x59) else {
            panic!("{name}");
        };
        assert_eq!((l.name.as_str(), l.tab_order, l.flags), ("Layout1", 1, 1));
        assert_eq!(l.block_record, Handle(0x58), "{name}");
        assert_eq!(l.active_viewport, Handle(0x240), "{name}");
        assert_eq!(l.viewports, [Handle(0x240), Handle(0x245)], "{name}");
        assert_eq!(l.limits_max, [10.74999951940822, 8.249999763458733]);
        let p = &l.plot;
        assert_eq!(p.printer, "none_device", "{name}");
        assert_eq!(p.paper_size, "Letter_(8.50_x_11.00_Inches)", "{name}");
        assert_eq!(
            (p.flags, p.rotation, p.plot_type, p.scale_type),
            (688, 1, 5, 16)
        );
        assert_eq!(
            (p.shade_plot_mode, p.shade_plot_resolution, p.shade_plot_dpi),
            (0, 2, 300),
            "{name}"
        );
        // MLEADERSTYLE E5 ("Annotative" in ACAD_MLEADERSTYLE): annotative
        // (296 = 1, 295 = 0), so the annotative bit is stored before the
        // property-changed bit (ODS: after). The text angle type (175) and align
        // left (297) fields are stored in every version (ODS: only when new
        // format); before R2010 the style carries the ACAD_MLEADERVER mark.
        // R2010 fields read before R2010 take the R2010 file's values.
        let NonGraphical::MLeaderStyle(m) = object(&d, 0xE5) else {
            panic!("{name}");
        };
        // Before R2010 the style is marked by ACAD_MLEADERVER extended data
        // (its format is chosen from that mark).
        if v < Version::R2010 {
            let marker = opendwg::object::mleader_style_marker(v, d.objects())
                .unwrap_or_else(|| panic!("{name}: no ACAD_MLEADERVER"));
            let xdata = &d.get(Handle(0xE5)).unwrap().object_common().xdata;
            assert!(xdata.iter().any(|x| x.app == marker), "{name}");
            // R2010 and later need no marker.
            assert_eq!(
                opendwg::object::mleader_style_marker(Version::R2010, d.objects()),
                None
            );
            // An external caller of decode_object gets the same style from
            // the marker the helper finds.
            let bytes = std::fs::read(&path).unwrap();
            let raw = opendwg::container::read(&bytes).unwrap();
            let original = &common::object_bytes(&bytes)[&Handle(0xE5)];
            let mut ctx =
                DecodeCtx::new(v, raw.codepage, &d.classes).with_mleader_style_marker(Some(marker));
            let again = decode_object(original, Handle(0xE5), &mut ctx).unwrap();
            assert_eq!(&again, d.get(Handle(0xE5)).unwrap(), "{name}");
        }
        assert_eq!(m.description, "Standard", "{name}");
        assert_eq!((m.version, m.content_type, m.leader_type), (2, 2, 1));
        assert_eq!(m.line_color, Color::ByBlock, "{name}");
        assert_eq!((m.line_type, m.text_style), (Handle(0x14), Handle(0x11)));
        assert_eq!((m.landing_gap, m.landing_distance), (0.09, 0.36));
        assert_eq!((m.text_angle_type, m.align_text_left), (1, false));
        assert_eq!((m.annotative, m.property_changed), (true, false), "{name}");
        assert_eq!(m.break_size, 0.125, "{name}");
        assert_eq!(
            (
                m.attachment_direction,
                m.top_attachment,
                m.bottom_attachment
            ),
            (0, 9, 9),
            "{name}"
        );
        // TABLESTYLE 87 "Standard".
        match object(&d, 0x87) {
            NonGraphical::TableStyle(TableStyle::Legacy(t)) => {
                assert!(v < Version::R2010, "{name}");
                assert_eq!(t.description, "Standard");
                assert_eq!((t.horizontal_margin, t.vertical_margin), (0.06, 0.06));
                let heights: Vec<f64> = t.rows.iter().map(|r| r.text_height).collect();
                assert_eq!(heights, [0.18, 0.25, 0.18], "{name}");
                let aligns: Vec<i16> = t.rows.iter().map(|r| r.alignment).collect();
                assert_eq!(aligns, [2, 5, 5], "{name}");
                for r in &t.rows {
                    // In R2000 too these are R2004 CMC colors (TABLESTYLE is
                    // newer than R2000).
                    assert_eq!(r.text_style, Handle(0x11), "{name}");
                    assert_eq!(
                        (&r.text_color, &r.fill_color),
                        (&Color::ByBlock, &Color::Index(7))
                    );
                    assert!(!r.fill_enabled, "{name}");
                    assert!(r.borders.iter().all(|b| b.line_weight == -2 && b.visible));
                    assert_eq!(r.data_type, 512, "{name}");
                }
            }
            NonGraphical::TableStyle(TableStyle::Cells(t)) => {
                assert!(v >= Version::R2010, "{name}");
                assert_eq!(t.description, "Standard");
                assert_eq!(t.unknown_bl, [0, 101], "{name}");
                assert_eq!(t.table_style.name, "Table", "{name}");
                // No background: CMC color method 0xC8, not in ODS.
                let data = t.table_style.style.data.as_ref().unwrap();
                assert_eq!(data.background_color, Color::None, "{name}");
                assert_eq!(data.content.text_height, 0.18, "{name}");
                let names: Vec<(i32, i32, &str)> = t
                    .cell_styles
                    .iter()
                    .map(|(_, s)| (s.id, s.class, s.name.as_str()))
                    .collect();
                assert_eq!(
                    names,
                    [(1, 1, "_TITLE"), (2, 1, "_HEADER"), (3, 2, "_DATA")],
                    "{name}"
                );
            }
            o => panic!("{name}: {o:?}"),
        }
        // DICTIONARYVAR F0: "1:1", a text field (ODS: BS).
        let NonGraphical::DictionaryVar(x) = object(&d, 0xF0) else {
            panic!("{name}");
        };
        assert_eq!((x.schema, x.value.as_str()), (0, "1:1"), "{name}");
        // SCALE B7 "1:1".
        let NonGraphical::Scale(x) = object(&d, 0xB7) else {
            panic!("{name}");
        };
        assert_eq!(x.name, "1:1", "{name}");
        assert!(x.paper_units == 1.0 && x.drawing_units == 1.0 && x.unit_scale);
        // GROUP 69E: unnamed, selectable, six entities.
        let NonGraphical::Group(x) = object(&d, 0x69E) else {
            panic!("{name}");
        };
        assert_eq!((x.name.as_str(), x.unnamed, x.selectable), ("", 1, 1));
        let want: Vec<Handle> = (0x698..=0x69D).rev().map(Handle).collect();
        assert_eq!(x.entities, want, "{name}");
        // IMAGEDEF 8C4, its reactor 8C5, RASTERVARIABLES 8C3 and
        // WIPEOUTVARIABLES 44E (not in ODS).
        let NonGraphical::ImageDef(x) = object(&d, 0x8C4) else {
            panic!("{name}");
        };
        assert_eq!(x.path, ".\\image.JPG", "{name}");
        assert_eq!(
            (x.size, x.loaded, x.resolution_units),
            ([994.0, 965.0], true, 5)
        );
        assert_eq!(x.pixel_size, [0.2645833333333333; 2], "{name}");
        assert!(matches!(
            object(&d, 0x8C5),
            NonGraphical::ImageDefReactor(r) if r.class_version == 2
        ));
        assert!(matches!(
            object(&d, 0x8C3),
            NonGraphical::RasterVariables(r)
                if (r.class_version, r.display_frame, r.display_quality, r.units) == (0, 1, 1, 5)
        ));
        assert!(matches!(
            object(&d, 0x44E),
            NonGraphical::WipeoutVariables(w) if w.display_frame == 1
        ));
        // SORTENTSTABLE 355: model space; entity 353 sorts as 35A, 35A as
        // 354.
        let NonGraphical::SortEntsTable(x) = object(&d, 0x355) else {
            panic!("{name}");
        };
        assert_eq!(x.block, Handle(0x1F), "{name}");
        assert_eq!(
            x.entries[..2],
            [
                (Handle(0x35A), Handle(0x353)),
                (Handle(0x354), Handle(0x35A))
            ],
            "{name}"
        );
        // SPATIAL_FILTER D32: the kept bits are the same in every version.
        let NonGraphical::SpatialFilter(x) = object(&d, 0xD32) else {
            panic!("{name}");
        };
        assert_eq!(x.bits.strings, None, "{name}");
        match &filter {
            None => filter = Some(x.clone()),
            Some(f) => assert_eq!(f, x, "{name}"),
        }
    }
    assert!(seen == 0 || seen == 6, "{seen}");
}
