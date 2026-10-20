//! Corpus checks of the typed symbol tables (Task 13): every table object
//! decodes typed, the counts match the manifest, R2018 objects re-encode bit
//! for bit, and values that pin ODS deviations are asserted on real data.

mod common;

use std::collections::BTreeMap;

use opendwg::bits::Color;
use opendwg::object::{
    BlockRecord, DecodeCtx, EntityList, Layer, NonGraphical, Object, TableKind, decode_object,
    encode_object,
};
use opendwg::{Drawing, Handle, Version};

/// Type names of the table objects.
fn is_table_type(name: &str) -> bool {
    TableKind::ALL.into_iter().any(|k| {
        [k.control_code(), k.entry_code()]
            .into_iter()
            .any(|c| opendwg::object::fixed_type(c).map(|(n, _)| n) == Some(name))
    })
}

/// The manifest key counting objects of type `name`.
fn count_key(name: &str) -> String {
    if name.ends_with("_CONTROL") {
        "table_control_count".into()
    } else {
        format!("{}_count", name.to_ascii_lowercase())
    }
}

fn typed(d: &Drawing) -> impl Iterator<Item = (Handle, &NonGraphical)> {
    d.objects().filter_map(|(h, o)| match o {
        Object::Other { object, .. } => Some((h, object)),
        _ => None,
    })
}

#[test]
fn corpus_tables_decode() {
    for (e, path) in common::fetched_files_or_skip() {
        let name = &e.name;
        let d = Drawing::open(&path).unwrap();
        let fallbacks: Vec<_> = d
            .diagnostics()
            .iter()
            .filter(|x| x.message.contains("kept raw"))
            .collect();
        assert!(fallbacks.is_empty(), "{name}: {fallbacks:?}");
        let mut counts: BTreeMap<String, i64> = BTreeMap::new();
        for (h, obj) in d.objects() {
            let t = obj.type_name();
            if !is_table_type(&t) {
                continue;
            }
            assert!(obj.raw().is_none(), "{name}: {h} {t} kept raw");
            *counts.entry(count_key(&t)).or_default() += 1;
        }
        for key in [
            "table_control_count",
            "block_header_count",
            "layer_count",
            "style_count",
            "ltype_count",
            "view_count",
            "ucs_count",
            "vport_count",
            "appid_count",
            "dimstyle_count",
            "vp_ent_hdr_count",
        ] {
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
fn corpus_tables_r2018_bytes() {
    let files = common::fetched_files_for("AC1032");
    for (e, path) in files {
        let name = &e.name;
        let bytes = std::fs::read(&path).unwrap();
        let d = Drawing::from_bytes(&bytes).unwrap();
        let originals = common::object_bytes(&bytes);
        // Per type: objects, and those whose bytes differ but decode equal.
        let mut stats: BTreeMap<String, (usize, usize)> = BTreeMap::new();
        for (h, obj) in d.objects() {
            let t = obj.type_name().into_owned();
            if !is_table_type(&t) {
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
        assert!(stats.len() >= 15, "{name}: only {stats:?}");
    }
}

fn layers(d: &Drawing) -> BTreeMap<String, Layer> {
    typed(d)
        .filter_map(|(_, o)| match o {
            NonGraphical::Layer(l) => Some((l.entry.name.clone(), l.clone())),
            _ => None,
        })
        .collect()
}

fn blocks(d: &Drawing) -> Vec<(Handle, BlockRecord)> {
    typed(d)
        .filter_map(|(h, o)| match o {
            NonGraphical::BlockRecord(b) => Some((h, b.clone())),
            _ => None,
        })
        .collect()
}

/// Layer flags, lineweights and colors of the sample drawing, read from every
/// version, against the values of its DXF export (`sample_AC1032_ascii.dxf`
/// and `sample_AC1015_ascii.dxf`): 70 flags, 62 color, 420 true color, 430
/// color book, 290 plot, 370 lineweight.
#[test]
fn corpus_layer_values() {
    let mut seen = 0;
    for (e, path) in common::fetched_files_or_skip() {
        if !e.name.starts_with("acadsharp-sample-") {
            continue;
        }
        seen += 1;
        let d = Drawing::open(&path).unwrap();
        let v = d.source_version().unwrap();
        let l = layers(&d);
        let name = &e.name;
        let get = |n: &str| l.get(n).unwrap_or_else(|| panic!("{name}: no layer {n}"));
        for (n, layer) in &l {
            let off = n == "Layer_Off";
            assert_eq!(layer.off, off, "{name}: {n} off");
            assert_eq!(layer.frozen, n == "Layer_Freeze", "{name}: {n} frozen");
            assert_eq!(layer.locked, n == "Layer_Lock", "{name}: {n} locked");
            assert_eq!(
                layer.frozen_in_new_viewports,
                n == "Layer_vp_freeze",
                "{name}: {n} vp freeze"
            );
            let plot = !matches!(n.as_str(), "Layer_NoPlot" | "Defpoints");
            assert_eq!(layer.plot, plot, "{name}: {n} plot");
            // Index into the standard lineweights: 9 is 0.35 mm, 11 is 0.50
            // mm, 31 is Default (DXF -3).
            let lw = match n.as_str() {
                "Layer_lw_035" => 9,
                "Layer_lw_050" => 11,
                _ => 31,
            };
            assert_eq!(layer.lineweight, lw, "{name}: {n} lineweight");
            assert_eq!(layer.entry.xref_index, 0, "{name}: {n}");
            assert!(!layer.entry.xref_dependent, "{name}: {n}");
            assert_eq!(layer.entry.xref_block, Handle(0), "{name}: {n}");
            assert_eq!(layer.plotstyle, Handle(0xF), "{name}: {n} plot style");
        }
        assert_eq!(get("Layer_color_80").color, Color::Index(80), "{name}");
        assert_eq!(get("Layer2").color, Color::Index(65), "{name}");
        assert_eq!(get("Layer_Off").color, Color::Index(7), "{name}");
        let ltype_of = |n: &str| get(n).linetype;
        assert_ne!(ltype_of("Layer_Lt_dash"), ltype_of("Layer1"), "{name}");
        if v >= Version::R2004 {
            // 420 = 1151726, 8200527 and 16564615; 430 = "DIC COLOR GUIDE(R)$DIC 7".
            assert_eq!(
                get("Layer_true_color").color,
                Color::Rgb {
                    r: 0x11,
                    g: 0x92,
                    b: 0xEE
                },
                "{name}"
            );
            assert_eq!(
                get("color_125_33_79").color,
                Color::Rgb {
                    r: 125,
                    g: 33,
                    b: 79
                },
                "{name}"
            );
            assert_eq!(
                get("Layer_color_book").color,
                Color::Book {
                    rgb: 0xFC_C1_87,
                    color_name: "DIC 7".into(),
                    book_name: "DIC COLOR GUIDE(R)".into(),
                },
                "{name}"
            );
        } else {
            // R2000 keeps the nearest ACI color (DXF 62).
            assert_eq!(get("Layer_true_color").color, Color::Index(150), "{name}");
            assert_eq!(get("color_125_33_79").color, Color::Index(247), "{name}");
            assert_eq!(get("Layer_color_book").color, Color::Index(31), "{name}");
        }
        if v >= Version::R2007 {
            assert_eq!(get("0").material, Handle(0x98), "{name}");
        } else {
            assert_eq!(get("0").material, Handle(0), "{name}");
        }
    }
    if seen == 0 {
        common::skip("acadsharp-sample files");
    }
}

/// Other table values from the DXF export of the sample drawing.
#[test]
fn corpus_table_values() {
    let mut seen = 0;
    for (e, path) in common::fetched_files_or_skip() {
        if !e.name.starts_with("acadsharp-sample-") {
            continue;
        }
        seen += 1;
        let name = &e.name;
        let d = Drawing::open(&path).unwrap();
        let v = d.source_version().unwrap();
        let mut styles = vec![];
        let mut ltypes = BTreeMap::new();
        for (_, o) in typed(&d) {
            match o {
                NonGraphical::TextStyle(s) => styles.push(s.clone()),
                NonGraphical::Linetype(l) => {
                    ltypes.insert(l.entry.name.clone(), l.clone());
                }
                NonGraphical::TableControl(c) => {
                    let extra = match c.kind {
                        TableKind::Block | TableKind::Linetype => 2,
                        TableKind::DimStyle => 4,
                        _ => 0,
                    };
                    assert_eq!(c.extra.len(), extra, "{name}: {:?}", c.kind);
                }
                _ => {}
            }
        }
        // STYLE: the two shape files (DXF 70 = 1) have empty names; the shape
        // bit comes first (ODS has it after the vertical bit).
        assert_eq!(styles.len(), 6, "{name}");
        for s in &styles {
            let shape = s.font_name.ends_with(".shx");
            assert_eq!(s.shape_file, shape, "{name}: {s:?}");
            assert!(!s.vertical, "{name}: {s:?}");
            assert_eq!(s.entry.name.is_empty(), shape, "{name}: {s:?}");
            assert_eq!(s.width_factor, 1.0, "{name}");
        }
        let standard = styles.iter().find(|s| s.entry.name == "Standard").unwrap();
        assert_eq!(standard.font_name, "arial.ttf", "{name}");
        assert_eq!(standard.last_height, 0.5, "{name}");

        // GAS_LINE: 3 dashes, the second draws "GAS" with style 0x11.
        let gas = &ltypes["GAS_LINE"];
        assert_eq!(gas.pattern_length, 0.95, "{name}");
        assert_eq!(gas.alignment, b'A', "{name}");
        let lens: Vec<f64> = gas.dashes.iter().map(|d| d.length).collect();
        assert_eq!(lens, [0.5, -0.2, -0.25], "{name}");
        let text = &gas.dashes[1];
        assert_eq!(text.style, Handle(0x11), "{name}");
        assert_eq!(text.offset, [-0.1, -0.05], "{name}");
        assert_eq!(text.scale, 0.1, "{name}");
        // DXF 74 is 2 in the R2000 export and 10 in the R2018 one.
        assert_eq!(text.flags & 0x7, 2, "{name}");
        assert_eq!(gas.text(1).as_deref(), Some("GAS"), "{name}");
        // TRACKS: a shape dash (74 = 4, 75 = 130) from style 0x27F.
        let tracks = &ltypes["TRACKS"].dashes[0];
        assert_eq!(
            (tracks.flags, tracks.shape_number, tracks.style),
            (4, 130, Handle(0x27F)),
            "{name}"
        );
        if v >= Version::R2007 {
            assert!(ltypes["TRACKS"].text_area.is_empty(), "{name}");
        }

        // BLOCK_RECORD: inserts (331), units (70) and scaling (281).
        let b = blocks(&d);
        let my_block = &b
            .iter()
            .find(|(_, b)| b.entry.name == "my_block")
            .unwrap()
            .1;
        // The DXF export lists 4 of the 10 inserts the DWG stores.
        assert_eq!(my_block.inserts.len(), 10, "{name}");
        for h in [0xD2F, 0xA43] {
            assert!(my_block.inserts.contains(&Handle(h)), "{name}: {h:X}");
        }
        if v >= Version::R2007 {
            assert_eq!(
                (my_block.insert_units, my_block.block_scaling),
                (4, 1),
                "{name}"
            );
        }
        let model = &b
            .iter()
            .find(|(_, b)| b.entry.name == "*Model_Space")
            .unwrap()
            .1;
        assert_eq!(model.layout, Handle(0x22), "{name}");
        assert!(b.iter().all(|(_, b)| b.explodable), "{name}");
    }
    if seen == 0 {
        common::skip("acadsharp-sample files");
    }
}

/// R2007+ table entries store one `BS` for the xref fields (ODS: `B`, `BS`,
/// `B`). Read the ODS way, every R2007+ entry would leave bits unread and
/// fall back to raw; read as one `BS` they decode to the last bit with the
/// values the R2000/R2004 files store (`xrefindex+1` 0, not dependent).
#[test]
fn corpus_r2007_xref_short() {
    let mut seen = 0;
    for (e, path) in common::fetched_files_or_skip() {
        let d = Drawing::open(&path).unwrap();
        if d.source_version().unwrap() < Version::R2007 {
            continue;
        }
        seen += 1;
        let mut entries = 0;
        for (h, o) in typed(&d) {
            let entry = match o {
                NonGraphical::BlockRecord(x) => &x.entry,
                NonGraphical::Layer(x) => &x.entry,
                NonGraphical::TextStyle(x) => &x.entry,
                NonGraphical::Linetype(x) => &x.entry,
                NonGraphical::View(x) => &x.entry,
                NonGraphical::Ucs(x) => &x.entry,
                NonGraphical::Vport(x) => &x.entry,
                NonGraphical::AppId(x) => &x.entry,
                NonGraphical::DimStyle(x) => &x.entry,
                NonGraphical::VpEntHdr(x) => &x.entry,
                _ => continue,
            };
            entries += 1;
            assert_eq!(
                (entry.xref_index, entry.xref_dependent, entry.xref_block),
                (0, false, Handle(0)),
                "{}: {h}",
                e.name
            );
        }
        assert!(entries > 30, "{}: {entries}", e.name);
    }
    if seen == 0 {
        common::skip("R2007+ files");
    }
}

/// R2000 block records store the first and last entity; the drawing turns
/// them into full lists. The R2000 and R2004 samples are one drawing with the
/// same handles, so the lists must equal the R2004 file's stored lists.
#[test]
fn corpus_r2000_block_entity_lists() {
    let (Some(r2000), Some(r2004)) = (
        common::corpus_file_or_skip("acadsharp-sample-ac1015"),
        common::corpus_file_or_skip("acadsharp-sample-ac1018"),
    ) else {
        return;
    };
    let a = Drawing::open(&r2000).unwrap();
    let b = Drawing::open(&r2004).unwrap();
    assert!(a.diagnostics().is_empty(), "{:?}", a.diagnostics());
    let lists = |d: &Drawing| -> BTreeMap<Handle, Vec<Handle>> {
        blocks(d)
            .into_iter()
            .map(|(h, b)| match b.entities {
                EntityList::Owned(v) => (h, v),
                EntityList::Linked { .. } => panic!("{h}: list not built"),
            })
            .collect()
    };
    let (la, lb) = (lists(&a), lists(&b));
    assert_eq!(la.len(), 27);
    // One anonymous block was renumbered between the two saves.
    let mut same = 0;
    let mut total = 0;
    for (h, list) in &la {
        if let Some(other) = lb.get(h) {
            assert_eq!(list, other, "{h}");
            same += 1;
            total += list.len();
        }
    }
    assert_eq!(same, 26);
    assert!(total > 300, "{total}");
    // With the lists built, the R2000 block records convert to R2018.
    for (h, _) in blocks(&a) {
        encode_object(a.get(h).unwrap(), &a.classes).unwrap();
    }
}
