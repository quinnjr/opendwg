//! Corpus checks of MATERIAL, VISUALSTYLE, ACDBSECTIONVIEWSTYLE and
//! ACDBDETAILVIEWSTYLE, which ODS does not describe: every object of these
//! types decodes typed from the version the library models it from, R2018
//! objects re-encode bit for bit, others re-decode to equal values, and
//! values found with the DXF exports beside the samples are pinned.

mod common;

use std::collections::BTreeMap;

use opendwg::bits::Color;
use opendwg::object::{
    DecodeCtx, NonGraphical, Object, VISUAL_STYLE_KINDS, VisualStyleValue, XDataItem,
    decode_object, encode_object,
};
use opendwg::{Drawing, Handle, Version};

/// The types, with the first version each is typed in.
const TYPES: [(&str, Version); 4] = [
    ("MATERIAL", Version::R2007),
    ("VISUALSTYLE", Version::R2013),
    ("ACDBSECTIONVIEWSTYLE", Version::R2007),
    ("ACDBDETAILVIEWSTYLE", Version::R2007),
];

#[test]
fn corpus_render_styles_decode_and_reencode() {
    let mut typed: BTreeMap<&str, usize> = BTreeMap::new();
    for (e, path) in common::fetched_files_or_skip() {
        let name = &e.name;
        let bytes = std::fs::read(&path).unwrap();
        let d = Drawing::from_bytes(&bytes).unwrap();
        let version = d.source_version().unwrap();
        let originals = (version == Version::R2018).then(|| common::object_bytes(&bytes));
        for (h, obj) in d.objects() {
            let t = obj.type_name();
            let Some(&(t, since)) = TYPES.iter().find(|(n, _)| *n == t) else {
                continue;
            };
            let marked = obj.object_common().xdata.iter().any(|a| {
                a.items
                    .iter()
                    .any(|i| matches!(i, XDataItem::String(s) if s == "AcDbSavedByObjectVersion"))
            });
            if version < since || (t == "VISUALSTYLE" && marked) {
                assert!(obj.raw().is_some(), "{name}: {h} {t} typed");
                assert_eq!(
                    obj.raw().unwrap().is_convertible_to(Version::R2018),
                    version >= Version::R2007
                );
                continue;
            }
            assert!(obj.raw().is_none(), "{name}: {h} {t} kept raw");
            *typed.entry(t).or_default() += 1;
            let out = encode_object(obj, &d.classes).unwrap();
            if let Some(o) = &originals {
                assert_eq!(out, o[&h], "{name}: {h} {t} not bit-identical");
            }
            let mut ctx = DecodeCtx::new(Version::R2018, 30, &d.classes);
            assert_eq!(
                &decode_object(&out, h, &mut ctx).unwrap(),
                obj,
                "{name}: {h} {t}"
            );
        }
    }
    if !typed.is_empty() {
        assert_eq!(typed.len(), 4, "{typed:?}");
    }
}

fn object(d: &Drawing, h: u64) -> &NonGraphical {
    match d.get(Handle(h)) {
        Some(Object::Other { object, .. }) => object,
        o => panic!("{h:X}: {o:?}"),
    }
}

/// Values of the template objects of the smallest R2018 file, as its DXF
/// export (the ACadSharp sample's sibling) lists them.
#[test]
fn corpus_render_style_values() {
    let Some(p) = common::corpus_file_or_skip("acadsharp-dynblock-lookup") else {
        return;
    };
    let d = Drawing::open(p).unwrap();
    let NonGraphical::Material(global) = object(&d, 0x98) else {
        panic!()
    };
    assert_eq!(global.name, "Global");
    assert_eq!(global.diffuse.method, 0);
    assert_eq!(global.diffuse_map.source, 1);
    assert_eq!(global.diffuse_map.file_name.as_deref(), Some(""));
    assert_eq!(global.channel_flags, 63);

    let NonGraphical::VisualStyle(wire) = object(&d, 0x9F) else {
        panic!()
    };
    assert_eq!(
        (wire.name.as_str(), wire.style_type, wire.internal),
        ("2dWireframe", 4, false)
    );
    assert_eq!(wire.properties.len(), VISUAL_STYLE_KINDS.len());
    // DXF: 90 0, 90 2, 90 1, 90 0, 40 0.6, 40 30, 62/420 white, ...
    assert_eq!(wire.properties[1], (VisualStyleValue::Int(2), 1));
    assert_eq!(wire.properties[4], (VisualStyleValue::Double(0.6), 1));
    assert_eq!(
        wire.properties[54].0,
        VisualStyleValue::Text("strokes_ogs.tif".into())
    );

    let NonGraphical::SectionViewStyle(s) = object(&d, 0x1AA) else {
        panic!()
    };
    assert_eq!(s.base.name, "Imperial24");
    assert_eq!(s.flags, 78);
    assert_eq!(s.identifier_style, Handle(0x11));
    assert_eq!(s.plane_linetype, Handle(0x16));
    assert_eq!((s.identifier_height, s.identifier_offset), (0.24, 0.48));
    assert_eq!((s.plane_lineweight, s.bend_lineweight), (25, 50));
    assert_eq!(s.identifier_exclude, "I, O, Q, S, X, Z");
    assert_eq!(s.hatch_pattern, "ANSI31");
    assert_eq!(s.hatch_background, Color::None);
    assert_eq!(s.hatch_angles.len(), 6);

    let NonGraphical::DetailViewStyle(v) = object(&d, 0x1AC) else {
        panic!()
    };
    assert_eq!(v.base.name, "Imperial24");
    assert_eq!((v.flags, v.identifier_placement), (3, 3));
    assert_eq!((v.identifier_offset, v.label_offset), (0.36, 0.75));
    assert_eq!(v.border_linetype, Handle(0x16));
}

/// The R2007-R2013 samples' section and detail view styles and materials
/// read as the R2018 sample's same objects (the fields older versions do
/// not store take the values AutoCAD 2018 gives them).
#[test]
fn corpus_older_render_styles_agree_with_r2018() {
    let Some(p) = common::corpus_file_or_skip("acadsharp-sample-ac1032") else {
        return;
    };
    let new = Drawing::open(p).unwrap();
    let mut checked = 0;
    for v in ["ac1021", "ac1024", "ac1027"] {
        let Some(p) = common::corpus_file_or_skip(&format!("acadsharp-sample-{v}")) else {
            continue;
        };
        let old = Drawing::open(p).unwrap();
        for (h, o) in old.objects() {
            let Object::Other { object, .. } = o else {
                continue;
            };
            if matches!(
                object,
                NonGraphical::SectionViewStyle(_)
                    | NonGraphical::DetailViewStyle(_)
                    | NonGraphical::Material(_)
            ) {
                let Some(Object::Other { object: want, .. }) = new.get(h) else {
                    panic!("{v}: {h}")
                };
                assert_eq!(object, want, "{v}: {h}");
                checked += 1;
            }
        }
    }
    assert!(checked == 0 || checked == 3 * 15, "{checked}");
}

/// A material the typed decoder cannot read (a procedural map) stays raw;
/// read from R2010 (whose MATERIAL layout is R2018's) it saves, its bits
/// copied unchanged.
#[test]
fn r2010_procedural_material_saves_raw() {
    use opendwg::object::{ObjWriter, encode_object_common};
    let Some(p) = common::corpus_file_or_skip("acadsharp-sample-ac1024") else {
        return;
    };
    let mut d = Drawing::open(p).unwrap();
    let (h, common) = d
        .objects()
        .find_map(|(h, o)| match o {
            Object::Other {
                common,
                object: NonGraphical::Material(_),
            } => Some((h, common.clone())),
            _ => None,
        })
        .unwrap();
    let class = d.classes.iter().find(|c| c.dxf_name == "MATERIAL").unwrap();
    // A material whose diffuse map is procedural (source 2), followed by
    // bits the layout does not describe.
    let mut w = ObjWriter::new(h);
    encode_object_common(&mut w, &common).unwrap();
    w.text("Procedural").unwrap();
    w.text("").unwrap();
    for _ in 0..2 {
        w.data.write_rc(0); // color method
        w.data.write_bd(1.0); // factor
    }
    w.data.write_bd(1.0); // blend
    for v in [2, 1, 1] {
        w.data.write_rc(v);
    }
    for i in 0..16 {
        w.data.write_bd(if i % 5 == 0 { 1.0 } else { 0.0 });
    }
    w.data.write_rc(2); // procedural
    w.data.write_bl(0x1234_5678);
    w.data.write_bd(0.25);
    let bytes = w.finish(class.class_number).unwrap();
    let mut ctx = DecodeCtx::new(Version::R2010, 30, &d.classes);
    let raw = decode_object(&bytes, h, &mut ctx).unwrap();
    assert!(raw.raw().is_some());
    *d.get_mut(h).unwrap() = raw.clone();
    assert!(
        d.unconvertible_objects().is_empty(),
        "{:?}",
        d.unconvertible_objects()
    );

    let out = d.to_bytes().unwrap();
    let back = Drawing::from_bytes(&out).unwrap();
    let r = back.get(h).unwrap().raw().unwrap();
    let want = raw.raw().unwrap();
    assert_eq!(
        (&r.data, r.data_bits, &r.strings, &r.handles),
        (&want.data, want.data_bits, &want.strings, &want.handles)
    );
    // The only finding: the material is kept raw again.
    let kept_raw = |x: &opendwg::Diagnostic| x.handle == Some(h) && x.message.contains("kept raw");
    assert!(
        back.diagnostics().iter().any(kept_raw),
        "{:?}",
        back.diagnostics()
    );
    assert!(
        back.diagnostics().iter().all(kept_raw),
        "{:?}",
        back.diagnostics()
    );
}
