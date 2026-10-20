//! Corpus checks of the hatches, viewports, images, wipeouts, ACIS
//! (modeler) entities, OLE frames and proxy entities (Task 17): every entity
//! of these types decodes typed, the counts match the manifest, R2018
//! entities re-encode bit for bit, older versions convert (or are refused
//! when R2018 cannot hold them), and values that pin ODS deviations are
//! asserted on real data.
//!
//! BODY, OLE2FRAME and ACAD_PROXY_ENTITY are not in the corpus; their unit
//! tests in `src/object/entities/misc.rs` are hermetic.

mod common;

use std::collections::{BTreeMap, BTreeSet};

use opendwg::bits::Color;
use opendwg::container::SectionName;
use opendwg::object::{
    AcisEntity, AcisGuid, AcisKind, AcisPayload, CLIP_RECTANGLE, DecodeCtx, Entity, GradientColor,
    Hatch, HatchEdge, Image, Object, PATH_DERIVED, PATH_POLYLINE, PathEdges, Viewport,
    decode_object, decrypt_sat, encode_object,
};
use opendwg::section::JulianDate;
use opendwg::section::acds::ACIS_SCHEMA;
use opendwg::{Drawing, Error, Handle, Version};

/// The Task 17 entity types, by type name, with their manifest count keys.
const TYPES: [(&str, &str); 9] = [
    ("HATCH", "hatch_count"),
    ("VIEWPORT", "viewport_count"),
    ("IMAGE", "image_count"),
    ("WIPEOUT", "wipeout_count"),
    ("REGION", "region_count"),
    ("3DSOLID", "solid3d_count"),
    ("BODY", "body_count"),
    ("OLE2FRAME", "ole2frame_count"),
    ("ACAD_PROXY_ENTITY", "proxy_entity_count"),
];

/// The types the corpus has.
const IN_CORPUS: usize = 6;

fn is_task_type(name: &str) -> bool {
    TYPES.iter().any(|(t, _)| *t == name)
}

fn r2018_ctx(d: &Drawing) -> DecodeCtx<'_> {
    DecodeCtx::new(Version::R2018, 30, &d.classes)
}

#[test]
fn corpus_hatch_misc_decode() {
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
fn corpus_hatch_misc_r2018_bytes() {
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
        assert_eq!(types.len(), IN_CORPUS, "{types:?}");
    }
}

/// Whether an entity is an ACIS entity read before R2013: the drawing
/// writer converts it (moving its data to AcDs), or refuses it (R2000 SAT).
fn is_old_acis(obj: &Object) -> bool {
    matches!(obj, Object::Entity { entity: Entity::Acis(a), .. } if a.payload.is_inline())
}

/// Every entity of these types read from an older version encodes as R2018
/// and decodes back to the same value. ACIS entities read before R2013 are
/// written by the drawing writer: alone, R2004-R2010 ones encode once
/// converted ([`AcisEntity::to_r2013`]); R2000 SAT is refused as
/// unconvertible.
#[test]
fn corpus_hatch_misc_older_versions_convert() {
    let mut seen: BTreeMap<Version, BTreeSet<String>> = BTreeMap::new();
    let mut refused: BTreeMap<Version, usize> = BTreeMap::new();
    let mut converted: BTreeMap<Version, usize> = BTreeMap::new();
    for (e, path) in common::fetched_files_or_skip() {
        let d = Drawing::open(&path).unwrap();
        let v = d.source_version().unwrap();
        for (h, obj) in d.objects() {
            let t = obj.type_name().into_owned();
            if !is_task_type(&t) {
                continue;
            }
            if is_old_acis(obj) {
                assert!(v < Version::R2013, "{}: {h} {t}", e.name);
                let Object::Entity {
                    common,
                    entity: Entity::Acis(a),
                } = obj
                else {
                    unreachable!()
                };
                if v == Version::R2000 {
                    assert!(!a.is_convertible() && a.to_r2013(AcisGuid::default()).is_none());
                    match encode_object(obj, &d.classes) {
                        Err(Error::UnconvertibleObjects(list)) => {
                            assert_eq!(list, [(h, t.clone())])
                        }
                        other => panic!("{}: {h} {t}: {other:?}", e.name),
                    }
                    *refused.entry(v).or_default() += 1;
                } else {
                    assert!(matches!(
                        encode_object(obj, &d.classes),
                        Err(Error::Encode(_))
                    ));
                    let (y, sab) = a.to_r2013(AcisGuid::derived("x", h)).unwrap();
                    assert!(sab.unwrap().starts_with(b"ACIS BinaryFile"));
                    let mut want = Object::Entity {
                        common: common.clone(),
                        entity: Entity::Acis(Box::new(y)),
                    };
                    if let Object::Entity { common, .. } = &mut want {
                        common.prev_next = None;
                    }
                    let out = encode_object(&want, &d.classes).unwrap();
                    let back = decode_object(&out, h, &mut r2018_ctx(&d)).unwrap();
                    assert_eq!(back, want, "{}: {h} {t}", e.name);
                    *converted.entry(v).or_default() += 1;
                }
                seen.entry(v).or_default().insert(t);
                continue;
            }
            let out = encode_object(obj, &d.classes)
                .unwrap_or_else(|err| panic!("{}: {h} {t}: {err}", e.name));
            let mut ctx = r2018_ctx(&d);
            let back = decode_object(&out, h, &mut ctx).unwrap();
            assert!(ctx.diagnostics.is_empty(), "{}: {h}", e.name);
            let mut want = obj.clone();
            if let Object::Entity { common, entity } = &mut want {
                common.prev_next = None;
                // R2004+ viewports store no VP_ENT_HDR.
                if let Entity::Viewport(vp) = entity {
                    vp.vp_ent_header = None;
                }
            }
            assert_eq!(back, want, "{}: {h} {t}", e.name);
            seen.entry(v).or_default().insert(t);
        }
    }
    for (v, types) in &seen {
        assert_eq!(types.len(), IN_CORPUS, "{v:?}: {types:?}");
    }
    // The 3 ACIS entities of each R2000-R2010 sample.
    for (v, n) in refused {
        assert_eq!(v, Version::R2000);
        assert_eq!(n, 3, "{v:?}");
    }
    for (v, n) in &converted {
        assert!((Version::R2004..Version::R2013).contains(v), "{v:?}");
        assert_eq!(*n, 3, "{v:?}");
    }
    if !converted.is_empty() {
        assert_eq!(converted.len(), 3);
    }
}

const SAVED: JulianDate = JulianDate {
    day: 2_461_300,
    ms: 43_200_000,
};

/// The R2007 and R2010 samples' ACIS entities save as R2018: each reads
/// back in the R2013+ form, with DXF 290 set, the GUID derived from the
/// drawing's fingerprint and its handle, and an AcDs record keyed by its
/// handle holding its SAB verbatim. The wireframe, history and unknown
/// field are kept. Saving is deterministic, and the converted drawing
/// saves again with its data storage copied. R2000 SAT stays
/// unconvertible.
#[test]
fn corpus_older_acis_saves_as_r2018() {
    let mut done = 0;
    for (e, path) in common::fetched_files_or_skip() {
        let name = &e.name;
        if !name.starts_with("acadsharp-sample-") {
            continue;
        }
        let d = Drawing::open(&path).unwrap();
        let v = d.source_version().unwrap();
        let handles = [0xD65, 0xD69, 0xD6A];
        if v == Version::R2000 {
            let refused: Vec<Handle> = d
                .unconvertible_objects()
                .into_iter()
                .filter(|(h, _)| handles.contains(&h.0))
                .map(|(h, _)| h)
                .collect();
            assert_eq!(refused.len(), 3, "{name}");
            continue;
        }
        if !(Version::R2007..Version::R2013).contains(&v) {
            continue;
        }
        assert!(d.unconvertible_objects().is_empty(), "{name}");
        assert!(d.data_storage.is_none(), "{name}");
        let bytes = d.to_bytes_at(SAVED).unwrap();
        assert_eq!(bytes, d.to_bytes_at(SAVED).unwrap(), "{name}");
        let back = Drawing::from_bytes(&bytes).unwrap();
        assert!(
            back.diagnostics().is_empty(),
            "{name}: {:?}",
            back.diagnostics()
        );
        let storage = back.data_storage.as_ref().unwrap();
        for h in handles {
            let (x, y) = (acis(&d, h), acis(&back, h));
            let AcisPayload::Inline { data, .. } = &x.payload else {
                panic!("{name}: {h:X}");
            };
            assert_eq!(
                y.payload,
                AcisPayload::DataStorage {
                    flag_290: true,
                    guid: AcisGuid::derived(&d.header.fingerprintguid, Handle(h)),
                    unknown: 0,
                },
                "{name}: {h:X}"
            );
            assert_eq!(
                storage.acis_data(Handle(h)),
                Some(data.as_slice()),
                "{name}"
            );
            assert_eq!(back.acis_data(Handle(h)), d.acis_data(Handle(h)), "{name}");
            assert_eq!(
                (&y.wireframe, y.wireframe_bits, y.unknown, y.history, y.kind),
                (&x.wireframe, x.wireframe_bits, x.unknown, x.history, x.kind),
                "{name}: {h:X}"
            );
        }
        // Saved again, the section read is written as read.
        let raw = opendwg::container::read(&bytes).unwrap();
        let again = opendwg::container::read(&back.to_bytes_at(SAVED).unwrap()).unwrap();
        assert_eq!(
            again.sections[&SectionName::AcDsPrototype],
            raw.sections[&SectionName::AcDsPrototype],
            "{name}"
        );
        done += 1;
    }
    if done > 0 {
        assert_eq!(done, 2);
    }
}

/// The R2013 and R2018 samples hold the same drawing: their entities of
/// these types decode equal.
#[test]
fn corpus_r2013_and_r2018_hatch_misc_agree() {
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
        let y = b.get(h).unwrap_or_else(|| panic!("{h} missing in R2018"));
        let (Object::Entity { entity: x, .. }, Object::Entity { entity: y, .. }) = (x, y) else {
            panic!("{h}: not entities");
        };
        assert_eq!(x, y, "{h}");
        n += 1;
    }
    assert_eq!(n, 19);
}

fn entity(d: &Drawing, h: u64) -> &Entity {
    match d.get(Handle(h)) {
        Some(Object::Entity { entity, .. }) => entity,
        o => panic!("{h:X}: {o:?}"),
    }
}

fn hatch(d: &Drawing, h: u64) -> &Hatch {
    match entity(d, h) {
        Entity::Hatch(x) => x,
        e => panic!("{h:X}: {e:?}"),
    }
}

fn acis(d: &Drawing, h: u64) -> &AcisEntity {
    match entity(d, h) {
        Entity::Acis(x) => x,
        e => panic!("{h:X}: {e:?}"),
    }
}

/// Values of the sample drawing's hatches, read from every version, against
/// its DXF exports (`sample_AC10xx_ascii.dxf`).
#[test]
fn corpus_hatch_values() {
    let mut seen = 0;
    for (e, path) in common::fetched_files_or_skip() {
        if !e.name.starts_with("acadsharp-sample-") {
            continue;
        }
        seen += 1;
        let name = &e.name;
        let d = Drawing::open(&path).unwrap();
        let v = d.source_version().unwrap();
        // HATCH 35A: ANSI31, associative with one edge path of 4 lines made
        // from 353, predefined, one pattern line at 45 degrees.
        let x = hatch(&d, 0x35A);
        assert_eq!(x.pattern_name, "ANSI31", "{name}");
        assert!(x.associative && !x.is_solid(), "{name}");
        assert_eq!((x.style, x.pattern_type), (1, 1), "{name}");
        assert_eq!(x.paths.len(), 1, "{name}");
        let p = &x.paths[0];
        assert_eq!(p.flags, 1, "{name}");
        assert_eq!(p.boundary_objects, [Handle(0x353)], "{name}");
        let PathEdges::Edges(edges) = &p.edges else {
            panic!("{name}: {:?}", p.edges);
        };
        assert_eq!(edges.len(), 4, "{name}");
        let HatchEdge::Line { start, end } = edges[0] else {
            panic!("{name}: {:?}", edges[0]);
        };
        assert_eq!(start, [102.74576160289178, 0.08531779347757151], "{name}");
        assert_eq!(end, [123.19908748479031, 0.08531779347757151], "{name}");
        let pat = x.pattern.as_ref().unwrap();
        assert_eq!(
            (pat.angle, pat.scale, pat.double),
            (0.0, 1.0, false),
            "{name}"
        );
        assert_eq!(pat.lines.len(), 1, "{name}");
        let l = &pat.lines[0];
        assert!((l.angle.to_degrees() - 45.0).abs() < 1e-12, "{name}");
        assert!((l.base[0] - 2.750381723718078).abs() < 1e-12, "{name}");
        assert!((l.offset[0] + 0.0883883476483185).abs() < 1e-12, "{name}");
        assert!((l.offset[1] - 0.0883883476483185).abs() < 1e-12, "{name}");
        assert!(l.dashes.is_empty(), "{name}");
        assert_eq!(x.seed_points.len(), 1, "{name}");
        // HATCH 371 and 376: solid gradient fills (DXF 450 = 1), LINEAR and
        // SPHERICAL, from blue (63 = 5, 421 = 255) to yellow (63 = 2, 421 =
        // 16776960). The gradient colors are CMC true colors.
        for (h, gradient) in [(0x371, "LINEAR"), (0x376, "SPHERICAL")] {
            let x = hatch(&d, h);
            assert!(x.is_solid(), "{name}");
            assert_eq!(x.pattern_name, "SOLID,_O", "{name}");
            if v < Version::R2004 {
                assert_eq!(x.gradient.enabled, 0, "{name}");
                assert!(x.gradient.colors.is_empty(), "{name}");
                continue;
            }
            assert_eq!(x.gradient.enabled, 1, "{name}");
            assert_eq!(x.gradient.name, gradient, "{name}");
            assert_eq!(
                x.gradient.colors,
                [
                    GradientColor {
                        value: 0.0,
                        color: Color::Rgb { r: 0, g: 0, b: 255 }
                    },
                    GradientColor {
                        value: 1.0,
                        color: Color::Rgb {
                            r: 255,
                            g: 255,
                            b: 0
                        }
                    }
                ],
                "{name}"
            );
        }
        // HATCH BBB: a custom (76 = 2) pattern of 78 lines at scale 0.1 in a
        // closed, bulge-free polyline path with flags 7 (derived), so the
        // pixel size (47) is stored; not associative, no boundary objects.
        let x = hatch(&d, 0xBBB);
        assert_eq!(x.pattern_type, 2, "{name}");
        assert!(!x.associative, "{name}");
        let pat = x.pattern.as_ref().unwrap();
        assert_eq!((pat.scale, pat.lines.len()), (0.1, 78), "{name}");
        assert_eq!(pat.lines[0].dashes.len(), 2, "{name}");
        let p = &x.paths[0];
        assert_eq!(p.flags, 1 | PATH_POLYLINE | PATH_DERIVED, "{name}");
        assert!(p.boundary_objects.is_empty(), "{name}");
        let PathEdges::Polyline(pl) = &p.edges else {
            panic!("{name}: {:?}", p.edges);
        };
        assert!(pl.closed && !pl.has_bulges, "{name}");
        assert_eq!(pl.vertices.len(), 4, "{name}");
        assert_eq!(
            pl.vertices[0].point,
            [18.38260550487516, -191.58379006621968]
        );
        assert!((x.pixel_size - 0.0491582611840805).abs() < 1e-12, "{name}");
    }
    assert!(seen == 0 || seen == 6, "{seen}");
}

/// Viewport, image and wipeout values against the DXF exports. The R2007+
/// viewport fields of R2000/R2004 viewports take what AutoCAD writes for
/// them in R2007 (the same viewports' values in the AC1021 sample).
#[test]
fn corpus_viewport_and_image_values() {
    for (e, path) in common::fetched_files_or_skip() {
        if !e.name.starts_with("acadsharp-sample-") {
            continue;
        }
        let name = &e.name;
        let d = Drawing::open(&path).unwrap();
        let v = d.source_version().unwrap();
        // VIEWPORT 240: status 819232 (90), circle zoom 1000 (72), lens 50
        // (42), grid major 5 (61), default lights (292 = 1, 282 = 1),
        // ambient 63 = 250 / 421 = 3355443 (0x333333), visual style 9F (348,
        // R2007+ only), no clip boundary or frozen layers.
        let Entity::Viewport(x) = entity(&d, 0x240) else {
            panic!("{name}");
        };
        let x: &Viewport = x;
        assert_eq!(x.status_flags, 819232, "{name}");
        assert_eq!((x.circle_zoom, x.lens_length), (1000, 50.0), "{name}");
        assert!((x.width - 24.42504922344277).abs() < 1e-12, "{name}");
        assert_eq!(x.grid_major, 5, "{name}");
        assert_eq!((x.use_default_lights, x.default_lighting_type), (true, 1));
        assert_eq!(
            x.ambient_color,
            Color::Rgb {
                r: 51,
                g: 51,
                b: 51
            },
            "{name}"
        );
        assert_eq!(x.clip_boundary, Handle(0), "{name}");
        assert!(x.frozen_layers.is_empty(), "{name}");
        let vs = if v >= Version::R2007 { 0x9F } else { 0 };
        assert_eq!(x.visual_style, Handle(vs), "{name}");
        assert_eq!(x.vp_ent_header.is_some(), v == Version::R2000, "{name}");
        // IMAGE 8C6: 994 x 965 pixels, flags 7, brightness/contrast 50, a
        // rectangular clip (71 = 1) of two corners, IMAGEDEF 8C4 and its
        // reactor 8C5.
        let Entity::Image(x) = entity(&d, 0x8C6) else {
            panic!("{name}");
        };
        let x: &Image = x;
        assert_eq!(x.size, [994.0, 965.0], "{name}");
        assert_eq!((x.display_flags, x.brightness, x.contrast), (7, 50, 50));
        assert_eq!(x.clip_type, CLIP_RECTANGLE, "{name}");
        assert_eq!(x.clip_boundary, [[-0.5, -0.5], [993.5, 964.5]], "{name}");
        assert_eq!((x.image_def, x.reactor), (Handle(0x8C4), Handle(0x8C5)));
        // WIPEOUT 44D, laid out as an IMAGE: a polygon (71 = 2) whose 4
        // vertices the DXF export lists closed (91 = 5, the first repeated);
        // no IMAGEDEF.
        let Entity::Wipeout(x) = entity(&d, 0x44D) else {
            panic!("{name}");
        };
        assert_eq!((x.clip_type, x.clipping), (2, true), "{name}");
        assert_eq!(x.clip_boundary.len(), 4, "{name}");
        assert_eq!(x.clip_boundary[0], [-0.5, 0.5], "{name}");
        assert_eq!(x.u_vector[0], 31.707742926072683, "{name}");
        assert_eq!((x.image_def, x.reactor), (Handle(0), Handle(0)), "{name}");
    }
}

/// The ACIS entities of every version: R2000 stores encrypted SAT blocks,
/// R2004-R2010 an inline SAB, R2013+ no data (its SAB is in the AcDs
/// section, a record keyed by the entity's handle) but a DXF 290 flag and a
/// GUID. The wireframe is the same bits
/// in every version.
#[test]
fn corpus_acis_values() {
    let mut wireframes: BTreeMap<u64, Vec<(Vec<u8>, u64)>> = BTreeMap::new();
    for (e, path) in common::fetched_files_or_skip() {
        if !e.name.starts_with("acadsharp-sample-") {
            continue;
        }
        let name = &e.name;
        let d = Drawing::open(&path).unwrap();
        let v = d.source_version().unwrap();
        for (h, kind) in [
            (0xD65, AcisKind::Solid3d),
            (0xD69, AcisKind::Region),
            (0xD6A, AcisKind::Solid3d),
        ] {
            let x = acis(&d, h);
            assert_eq!(x.kind, kind, "{name}");
            match &x.payload {
                AcisPayload::SatBlocks {
                    unknown,
                    data,
                    block_sizes,
                } => {
                    assert_eq!(v, Version::R2000, "{name}");
                    assert!(*unknown, "{name}");
                    let total: u32 = block_sizes.iter().sum();
                    assert_eq!(total as usize, data.len(), "{name}");
                    let sat = decrypt_sat(data);
                    assert!(sat.starts_with(b"400 "), "{name}");
                    let text = String::from_utf8_lossy(&sat);
                    assert!(text.contains("Autodesk AutoCAD"), "{name}");
                    // The blocks delimit the SAT: it ends with its last
                    // record, with no end-of-data marker.
                    assert!(sat.ends_with(b" #\r\n"), "{name}");
                    assert!(!text.contains("End-of-ACIS-data"), "{name}");
                }
                AcisPayload::Inline { unknown, data } => {
                    assert!((Version::R2004..Version::R2013).contains(&v), "{name}");
                    assert!(!unknown, "{name}");
                    assert!(data.starts_with(b"ACIS BinaryFile"), "{name}");
                    assert!(data.ends_with(b"\x0E\x03End\x0E\x02of\x0E\x04ACIS\x0D\x04data"));
                }
                AcisPayload::DataStorage {
                    flag_290,
                    guid,
                    unknown,
                } => {
                    assert!(v >= Version::R2013, "{name}");
                    // DXF 290 and 2, which ODS does not describe.
                    assert!(*flag_290, "{name}");
                    let want = match h {
                        0xD65 => "{1a113328-eb6d-d44d-824d-78b33668f9e7}",
                        0xD69 => "{3b0eb882-65ed-5e47-8593-6a36b15e7945}",
                        _ => "{2aff21bf-e73f-d14e-9b2e-7a1e6518d595}",
                    };
                    assert_eq!(guid.to_string(), want, "{name}");
                    assert_eq!(*unknown, 0, "{name}");
                    // The link: exactly one AcDs data record (ODS 24.2.2.3)
                    // under the ACIS schema names the entity.
                    let storage = d
                        .data_storage
                        .as_ref()
                        .unwrap_or_else(|| panic!("{name}: no AcDs"));
                    let schema = storage.schema_index(ACIS_SCHEMA).unwrap();
                    let records = storage
                        .records
                        .iter()
                        .filter(|r| r.schema == schema && r.handle == Handle(h))
                        .count();
                    assert_eq!(records, 1, "{name}: {h:X}");
                    assert!(
                        d.acis_data(Handle(h))
                            .unwrap()
                            .ends_with(b"ASM\x0D\x04data")
                    );
                }
                AcisPayload::Empty => panic!("{name}: {h:X} empty"),
            }
            assert_eq!(x.unknown, 0, "{name}");
            // R2007+: 3DSOLIDs store a history (hard owner, the next
            // handle); REGIONs none.
            let history = (v >= Version::R2007 && kind == AcisKind::Solid3d).then(|| Handle(h + 1));
            assert_eq!(x.history, history, "{name}: {h:X}");
            wireframes
                .entry(h)
                .or_default()
                .push((x.wireframe.clone(), x.wireframe_bits));
        }
    }
    for (h, w) in wireframes {
        assert!(w.windows(2).all(|p| p[0] == p[1]), "{h:X}");
        assert_eq!(w[0].1, if h == 0xD69 { 152 } else { 216 }, "{h:X}");
    }
}

/// An ACIS entity removed from an R2013+ drawing takes its data storage
/// record with it: the section is re-encoded (revision raised) without it,
/// every other record kept.
#[test]
fn removed_acis_entity_leaves_data_storage() {
    for name in ["acadsharp-sample-ac1027", "acadsharp-sample-ac1032"] {
        let Some(path) = common::corpus_file_or_skip(name) else {
            continue;
        };
        let mut d = Drawing::open(&path).unwrap();
        let before = d.data_storage.clone().unwrap();
        d.remove(Handle(0xD69)).unwrap();
        let back = Drawing::from_bytes(&d.to_bytes_at(SAVED).unwrap()).unwrap();
        assert!(
            back.diagnostics().is_empty(),
            "{name}: {:?}",
            back.diagnostics()
        );
        let after = back.data_storage.as_ref().unwrap();
        let mut want = before.clone();
        want.records.retain(|r| r.handle != Handle(0xD69));
        want.revision += 1;
        assert_eq!(after, &want, "{name}");
        assert_eq!(before.records.len(), want.records.len() + 1, "{name}");
        for h in [0xD65, 0xD6A] {
            assert_eq!(back.acis_data(Handle(h)), d.acis_data(Handle(h)), "{name}");
        }
    }
}

/// Records keyed by objects the library does not model as ACIS entities
/// survive a re-encode: only records of objects not written are dropped.
#[test]
fn records_of_other_objects_survive_reencode() {
    let Some(path) = common::corpus_file_or_skip("acadsharp-sample-ac1032") else {
        return;
    };
    let mut d = Drawing::open(&path).unwrap();
    let line = d
        .objects()
        .find(|(_, o)| o.type_name() == "LINE")
        .unwrap()
        .0;
    let storage = d.data_storage.as_mut().unwrap();
    storage.set_acis_data(line, b"kept".to_vec());
    d.remove(Handle(0xD69)).unwrap();
    let back = Drawing::from_bytes(&d.to_bytes_at(SAVED).unwrap()).unwrap();
    let after = back.data_storage.as_ref().unwrap();
    assert_eq!(after.acis_data(line), Some(&b"kept"[..]));
    assert_eq!(after.acis_data(Handle(0xD69)), None);
    // The thumbnails (schema 0) are all kept.
    let thumbnails =
        |s: &opendwg::section::DataStorage| s.records.iter().filter(|r| r.schema == 0).count();
    assert_eq!(
        thumbnails(after),
        thumbnails(d.data_storage.as_ref().unwrap())
    );
}

/// Every ACIS entity in the R2013+ form must have its data storage record:
/// without the section, or without a record, the write fails with a
/// diagnostic per entity. An entity converted without data has none by
/// design and saves.
#[test]
fn acis_links_without_records_are_refused() {
    if let Some(path) = common::corpus_file_or_skip("acadsharp-sample-ac1032") {
        let d = Drawing::open(&path).unwrap();
        let handles = |e: Error| -> Vec<Handle> {
            let Error::Invalid(diags) = e else {
                panic!("{e:?}")
            };
            diags.iter().map(|x| x.handle.unwrap()).collect()
        };
        let mut none = d.clone();
        none.data_storage = None;
        let e = none.to_bytes_at(SAVED).unwrap_err();
        assert_eq!(handles(e), [Handle(0xD65), Handle(0xD69), Handle(0xD6A)]);
        let mut missing = d.clone();
        missing
            .data_storage
            .as_mut()
            .unwrap()
            .retain_acis(|h| h != Handle(0xD6A));
        let e = missing.to_bytes_at(SAVED).unwrap_err();
        assert_eq!(handles(e), [Handle(0xD6A)]);
    }
    if let Some(path) = common::corpus_file_or_skip("acadsharp-sample-ac1024") {
        let mut d = Drawing::open(&path).unwrap();
        if let Some(Object::Entity {
            entity: Entity::Acis(a),
            ..
        }) = d.get_mut(Handle(0xD69))
        {
            a.payload = AcisPayload::Empty;
        }
        let back = Drawing::from_bytes(&d.to_bytes_at(SAVED).unwrap()).unwrap();
        assert!(matches!(
            acis(&back, 0xD69).payload,
            AcisPayload::DataStorage { flag_290: true, .. }
        ));
        assert_eq!(back.acis_data(Handle(0xD69)), None);
        assert!(back.acis_data(Handle(0xD65)).is_some());
    }
}

/// A data storage section that could not be decoded is written as read;
/// once ACIS entities were removed, the write diagnostics warn that it may
/// hold their records.
#[test]
fn undecoded_storage_warns_after_acis_removal() {
    let Some(path) = common::corpus_file_or_skip("acadsharp-sample-ac1032") else {
        return;
    };
    let mut raw = opendwg::container::read(&std::fs::read(&path).unwrap()).unwrap();
    raw.sections.get_mut(&SectionName::AcDsPrototype).unwrap()[0] ^= 0xFF;
    let mut d = Drawing::from_bytes(&opendwg::container::write_r2018(&raw).unwrap()).unwrap();
    assert!(d.data_storage.is_none());
    let warns = |d: &Drawing| {
        d.write_diagnostics()
            .iter()
            .filter(|x| x.message.contains("removed since"))
            .count()
    };
    assert_eq!(warns(&d), 0);
    d.to_bytes_at(SAVED).unwrap();
    d.remove(Handle(0xD69)).unwrap();
    assert_eq!(warns(&d), 1);
    d.to_bytes_at(SAVED).unwrap();
}

/// An entity converted from no data saves, reads back without a record,
/// and saves again (it held no record when read); the second read is the
/// same.
#[test]
fn empty_acis_resaves() {
    let Some(path) = common::corpus_file_or_skip("acadsharp-sample-ac1024") else {
        return;
    };
    let mut d = Drawing::open(&path).unwrap();
    if let Some(Object::Entity {
        entity: Entity::Acis(a),
        ..
    }) = d.get_mut(Handle(0xD69))
    {
        a.payload = AcisPayload::Empty;
    }
    let once = Drawing::from_bytes(&d.to_bytes_at(SAVED).unwrap()).unwrap();
    assert_eq!(once.acis_data(Handle(0xD69)), None);
    let twice = Drawing::from_bytes(&once.to_bytes_at(SAVED).unwrap()).unwrap();
    assert_eq!(acis(&twice, 0xD69), acis(&once, 0xD69));
    assert_eq!(twice.acis_data(Handle(0xD69)), None);
    assert_eq!(twice.data_storage, once.data_storage);
}

/// An R2018 entity without a record when read (here made by removing the
/// record and saving through the raw container) saves; one whose record
/// was there when read and is removed since is refused.
#[test]
fn recordless_r2018_entity_saves() {
    let Some(path) = common::corpus_file_or_skip("acadsharp-sample-ac1032") else {
        return;
    };
    let source = Drawing::open(&path).unwrap();
    let mut storage = source.data_storage.clone().unwrap();
    storage.retain_acis(|h| h != Handle(0xD6A));
    let mut raw = opendwg::container::read(&std::fs::read(&path).unwrap()).unwrap();
    raw.sections
        .insert(SectionName::AcDsPrototype, storage.encode().unwrap());
    let d = Drawing::from_bytes(&opendwg::container::write_r2018(&raw).unwrap()).unwrap();
    assert!(d.diagnostics().is_empty(), "{:?}", d.diagnostics());
    assert_eq!(d.acis_data(Handle(0xD6A)), None);
    let back = Drawing::from_bytes(&d.to_bytes_at(SAVED).unwrap()).unwrap();
    assert_eq!(back.acis_data(Handle(0xD6A)), None);
    assert_eq!(
        back.acis_data(Handle(0xD65)),
        source.acis_data(Handle(0xD65))
    );
    // A record there when read and removed since is still refused.
    let mut gone = d.clone();
    gone.data_storage
        .as_mut()
        .unwrap()
        .retain_acis(|h| h != Handle(0xD65));
    match gone.to_bytes_at(SAVED) {
        Err(Error::Invalid(diags)) => {
            let hs: Vec<Handle> = diags.iter().filter_map(|x| x.handle).collect();
            assert_eq!(hs, [Handle(0xD65)]);
        }
        other => panic!("{:?}", other.map(|b| b.len())),
    }
}

/// An R2013+ entity set to no data loses its record on save, so its old
/// data does not come back.
#[test]
fn emptied_r2018_entity_drops_its_record() {
    let Some(path) = common::corpus_file_or_skip("acadsharp-sample-ac1032") else {
        return;
    };
    let mut d = Drawing::open(&path).unwrap();
    if let Some(Object::Entity {
        entity: Entity::Acis(a),
        ..
    }) = d.get_mut(Handle(0xD65))
    {
        a.payload = AcisPayload::Empty;
    }
    let back = Drawing::from_bytes(&d.to_bytes_at(SAVED).unwrap()).unwrap();
    assert!(matches!(
        acis(&back, 0xD65).payload,
        AcisPayload::DataStorage { .. }
    ));
    assert_eq!(back.acis_data(Handle(0xD65)), None);
    assert_eq!(back.acis_data(Handle(0xD69)), d.acis_data(Handle(0xD69)));
    // Saved again, it stays without data.
    let again = Drawing::from_bytes(&back.to_bytes_at(SAVED).unwrap()).unwrap();
    assert_eq!(again.acis_data(Handle(0xD65)), None);
}

/// Whether `h` has a record in `d`'s data storage section.
fn has_ds_record(d: &Drawing, h: Handle) -> bool {
    d.data_storage
        .as_ref()
        .is_some_and(|s| s.records.iter().any(|r| r.handle == h))
}

/// Every object's `has_ds_binary_data` says whether it has a data storage
/// record, as AutoCAD writes it.
fn assert_ds_flags_match_records(d: &Drawing, name: &str) {
    for (h, obj) in d.objects() {
        assert_eq!(
            obj.object_common().has_ds_binary_data,
            has_ds_record(d, h),
            "{name}: {h} {}",
            obj.type_name()
        );
    }
}

/// The R2007 and R2010 samples' solids and regions, converted on save, read
/// back with `has_ds_binary_data` set (AutoCAD sets it on every R2013+ ACIS
/// entity with an AcDs record); every object's flag matches its record.
#[test]
fn converted_acis_sets_ds_flag() {
    let mut done = 0;
    for name in ["acadsharp-sample-ac1021", "acadsharp-sample-ac1024"] {
        let Some(path) = common::corpus_file_or_skip(name) else {
            continue;
        };
        let d = Drawing::open(&path).unwrap();
        let back = Drawing::from_bytes(&d.to_bytes_at(SAVED).unwrap()).unwrap();
        for h in [0xD65, 0xD69, 0xD6A] {
            let Some(Object::Entity { common, .. }) = back.get(Handle(h)) else {
                panic!("{name}: {h:X}");
            };
            assert!(common.obj.has_ds_binary_data, "{name}: {h:X}");
        }
        assert_ds_flags_match_records(&back, name);
        done += 1;
    }
    if done > 0 {
        assert_eq!(done, 2);
    }
}

/// An ACIS entity set to no data loses its record and its flag; one whose
/// flag is cleared by hand while it keeps its record gets it back.
#[test]
fn emptied_acis_clears_ds_flag() {
    let Some(path) = common::corpus_file_or_skip("acadsharp-sample-ac1032") else {
        return;
    };
    let mut d = Drawing::open(&path).unwrap();
    assert_ds_flags_match_records(&d, "read");
    if let Some(Object::Entity {
        entity: Entity::Acis(a),
        ..
    }) = d.get_mut(Handle(0xD65))
    {
        a.payload = AcisPayload::Empty;
    }
    if let Some(Object::Entity { common, .. }) = d.get_mut(Handle(0xD69)) {
        common.obj.has_ds_binary_data = false;
    }
    assert!(
        d.validate()
            .iter()
            .all(|x| !x.message.contains("data storage")),
        "{:?}",
        d.validate()
    );
    let back = Drawing::from_bytes(&d.to_bytes_at(SAVED).unwrap()).unwrap();
    let flag = |h: u64| {
        back.get(Handle(h))
            .unwrap()
            .object_common()
            .has_ds_binary_data
    };
    assert!(!flag(0xD65));
    assert!(flag(0xD69));
    assert_ds_flags_match_records(&back, "saved");
    // Converted from an R2010 entity without data: no record, no flag.
    let Some(path) = common::corpus_file_or_skip("acadsharp-sample-ac1024") else {
        return;
    };
    let mut d = Drawing::open(&path).unwrap();
    if let Some(Object::Entity {
        entity: Entity::Acis(a),
        ..
    }) = d.get_mut(Handle(0xD69))
    {
        a.payload = AcisPayload::Empty;
    }
    let back = Drawing::from_bytes(&d.to_bytes_at(SAVED).unwrap()).unwrap();
    assert!(
        !back
            .get(Handle(0xD69))
            .unwrap()
            .object_common()
            .has_ds_binary_data
    );
    assert_ds_flags_match_records(&back, "converted empty");
}
