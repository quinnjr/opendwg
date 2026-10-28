mod common;

use std::collections::BTreeSet;

use opendwg::container::{self, SectionName};
use opendwg::object::{
    DecodeCtx, Entity, NonGraphical, Object, TableStyle, decode_object, encode_object,
};
use opendwg::section::{classes, objmap};
use opendwg::{Drawing, Version};

#[test]
fn manifest_parses_and_covers_versions() {
    let m = common::load_manifest();
    if m.file.is_empty() {
        // No manifest (the published crate): `load_manifest` reported a skip.
        return;
    }
    let have: BTreeSet<String> = m.file.iter().map(|e| e.version.clone()).collect();
    for v in [
        Version::R2000,
        Version::R2004,
        Version::R2007,
        Version::R2010,
        Version::R2013,
        Version::R2018,
    ] {
        let magic = std::str::from_utf8(v.magic()).unwrap();
        assert!(have.contains(magic), "no corpus file for {magic}");
    }
    let mut names = BTreeSet::new();
    for e in &m.file {
        assert!(names.insert(&e.name), "duplicate name {}", e.name);
        assert!(
            e.url.starts_with("https://"),
            "{}: url must be https",
            e.name
        );
        assert!(
            e.url
                .split('/')
                .any(|seg| seg.len() == 40 && seg.bytes().all(|b| b.is_ascii_hexdigit())),
            "{}: url must be pinned to a 40-hex commit SHA",
            e.name
        );
        assert_eq!(e.sha256.len(), 64, "{}: bad sha256", e.name);
        assert!(e.sha256.bytes().all(|b| b.is_ascii_hexdigit()));
        assert!(!e.license_note.is_empty(), "{}: empty license_note", e.name);
        for f in &e.features {
            assert!(
                common::FEATURES.contains(&f.as_str()),
                "{}: unknown feature {f}",
                e.name
            );
        }
    }
}

#[test]
fn fetched_files_match_manifest_version_magic() {
    for (e, path) in common::fetched_files_or_skip() {
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(
            bytes.get(..6),
            Some(e.version.as_bytes()),
            "{}: file shorter than 6 bytes or magic differs from manifest",
            e.name
        );
    }
}

/// `expect.<key>` of a manifest entry as an integer.
fn expect_int(e: &common::Entry, key: &str) -> Option<i64> {
    e.expect.get(key).and_then(toml::Value::as_integer)
}

#[test]
fn every_corpus_file_reads() {
    for (e, path) in common::fetched_files_or_skip() {
        let name = &e.name;
        let bytes = std::fs::read(&path).unwrap();
        let d = Drawing::from_bytes(&bytes).unwrap_or_else(|err| panic!("{name}: {err}"));
        assert_eq!(
            d.source_version().map(|v| v.magic().to_vec()),
            Some(e.version.as_bytes().to_vec()),
            "{name}"
        );
        // AutoCAD-written samples read without a single diagnostic, which
        // also pins layouts that only warn when off (XDATA strings, header
        // and classes leftovers, code pages).
        assert!(d.diagnostics().is_empty(), "{name}: {:?}", d.diagnostics());

        let raw = container::read(&bytes).unwrap();
        let map = objmap::decode(&raw.sections[&SectionName::Handles]).unwrap();
        let skipped = d
            .diagnostics()
            .iter()
            .filter(|x| x.message.contains("skipped"))
            .count();
        let count = d.objects().count();
        assert_eq!(count, map.len() - skipped, "{name}: object count");
        for (h, obj) in d.objects() {
            assert_eq!(obj.handle(), h, "{name}");
            assert!(std::ptr::eq(d.get(h).unwrap(), obj));
        }
        let want = expect_int(&e, "object_count")
            .unwrap_or_else(|| panic!("{name}: manifest lacks expect.object_count"));
        assert_eq!(count as i64, want, "{name}: expect.object_count");
    }
}

#[test]
fn truncated_files_error_cleanly() {
    for (e, path) in common::fetched_files_or_skip() {
        let bytes = std::fs::read(&path).unwrap();
        let n = bytes.len();
        for cut in [n / 10, n / 2, n * 9 / 10, n - 1] {
            let prefix = &bytes[..cut];
            let outcome = std::panic::catch_unwind(|| Drawing::from_bytes(prefix));
            let result = outcome
                .unwrap_or_else(|_| panic!("{}: panic reading the first {cut} bytes", e.name));
            if let Ok(d) = result {
                assert!(
                    !d.diagnostics().is_empty(),
                    "{}: {cut} of {n} bytes read without a diagnostic",
                    e.name
                );
            }
        }
    }
}

#[test]
fn classes_resolve() {
    let mut custom = 0;
    for (e, path) in common::fetched_files_or_skip() {
        let d = Drawing::from_bytes(&std::fs::read(&path).unwrap()).unwrap();
        if e.features.iter().any(|f| f == "custom_objects") {
            custom += 1;
            assert!(!d.classes.is_empty(), "{}: no classes", e.name);
        }
        // Every class-numbered object resolves to its class by DXF name.
        // (Typed objects so far all have fixed type codes.)
        for (h, obj) in d.objects() {
            let Some(raw) = obj.raw() else {
                continue;
            };
            if raw.type_code >= 500 {
                let class = d
                    .classes
                    .iter()
                    .find(|c| c.class_number == raw.type_code)
                    .unwrap_or_else(|| panic!("{}: {h}: no class", e.name));
                assert_eq!(raw.class_name.as_deref(), Some(class.dxf_name.as_str()));
                assert_eq!(
                    matches!(obj, Object::Entity { .. }),
                    class.is_entity(),
                    "{}: {h} {}",
                    e.name,
                    class.dxf_name
                );
            } else {
                assert_eq!(raw.class_name, None);
            }
        }
    }
    if custom == 0 {
        common::skip("custom_objects files");
    }
}

#[test]
fn classes_reencode_r2018() {
    for (e, path) in common::fetched_files_or_skip() {
        let bytes = std::fs::read(&path).unwrap();
        let raw = container::read(&bytes).unwrap();
        let section = &raw.sections[&SectionName::Classes];
        let classes = classes::decode(section, raw.version, raw.codepage).unwrap();
        let out = classes::encode_r2018(&classes).unwrap();
        assert_eq!(
            classes::decode(&out, Version::R2018, 30).unwrap(),
            classes,
            "{}",
            e.name
        );
        if raw.version == Version::R2018 {
            assert_eq!(
                &out, section,
                "{}: R2018 classes not byte-identical",
                e.name
            );
        }
    }
}

/// Objects of R2018 files re-encode and decode back equal; R2000/R2004 raw
/// objects and R2000-R2010 ACIS entities refuse to be encoded alone as R2018 (the drawing writer
/// converts the R2004-R2010 ones). Typed objects of every older
/// version convert, and their R2018 form decodes back to the same values
/// (R2000 entity links aside: R2004+ entities have none).
#[test]
fn raw_objects_reencode() {
    for (e, path) in common::fetched_files_or_skip() {
        let d = Drawing::from_bytes(&std::fs::read(&path).unwrap()).unwrap();
        let v = d.source_version().unwrap();
        let mut typed = 0;
        for (h, obj) in d.objects() {
            let out = encode_object(obj, &d.classes);
            if v < Version::R2007 && obj.raw().is_some() {
                assert!(
                    matches!(out, Err(opendwg::Error::UnconvertibleObjects(_))),
                    "{}: {h}",
                    e.name
                );
                continue;
            }
            // ACIS entities read before R2013 are written by the drawing
            // writer (R2000 SAT not at all): alone, they do not encode.
            // Not a let chain: the crate's rust-version (1.85) has none.
            let inline_acis = match obj {
                Object::Entity {
                    entity: Entity::Acis(a),
                    ..
                } if a.payload.is_inline() => Some(a),
                _ => None,
            };
            if let Some(a) = inline_acis {
                let want_unconvertible = !a.is_convertible();
                assert_eq!(
                    matches!(out, Err(opendwg::Error::UnconvertibleObjects(_))),
                    want_unconvertible,
                    "{}: {h}",
                    e.name
                );
                assert!(out.is_err(), "{}: {h}", e.name);
                continue;
            }
            let out = out.unwrap_or_else(|err| panic!("{}: {h}: {err}", e.name));
            if v != Version::R2018 && obj.raw().is_some() {
                continue;
            }
            typed += usize::from(obj.raw().is_none());
            let mut ctx = DecodeCtx::new(Version::R2018, 30, &d.classes);
            let back = decode_object(&out, h, &mut ctx).unwrap();
            let mut want = obj.clone();
            if let Object::Entity { common, entity } = &mut want {
                common.prev_next = None;
                // R2018 leaders store no text box size.
                if let Entity::Leader(l) = entity {
                    l.box_size = None;
                }
                // R2004+ viewports store no VP_ENT_HDR.
                if let Entity::Viewport(vp) = entity {
                    vp.vp_ent_header = None;
                }
            }
            // Pre-R2010 table styles are written in their R2010 form.
            if let Object::Other {
                object: NonGraphical::TableStyle(t @ TableStyle::Legacy(_)),
                ..
            } = &mut want
            {
                let TableStyle::Legacy(l) = &*t else {
                    unreachable!()
                };
                *t = TableStyle::Cells(Box::new(l.to_r2010().unwrap()));
            }
            assert_eq!(back, want, "{}: {h} {}", e.name, obj.type_name());
            assert!(ctx.diagnostics.is_empty(), "{}: {h}", e.name);
        }
        assert!(typed > 50, "{}: {typed} typed objects", e.name);
    }
}

/// The object map of AutoCAD-written R2018 files re-encodes byte for byte.
/// AecObjects.dwg is left out: its writer splits pages differently (2019-,
/// 2031- and 17-byte pages) and leaves bytes after the end page.
#[test]
fn object_map_reencodes() {
    for (e, path) in common::fetched_files_or_skip() {
        let raw = container::read(&std::fs::read(&path).unwrap()).unwrap();
        let section = &raw.sections[&SectionName::Handles];
        let map = objmap::decode(section).unwrap();
        assert!(
            map.windows(2).all(|w| w[0].0 < w[1].0),
            "{}: unsorted",
            e.name
        );
        let out = objmap::encode(&map).unwrap();
        assert_eq!(objmap::decode(&out).unwrap(), map, "{}", e.name);
        if raw.version == Version::R2018 && e.name != "acadsharp-aec-objects" {
            assert_eq!(&out, section, "{}", e.name);
        }
    }
}

/// An R2004+ file cut just before the end, in its trailing header copy,
/// still has every section: it reads, with a warning that it was cut.
#[test]
fn missing_trailing_header_copy_is_a_warning() {
    let mut done = 0;
    for (e, path) in common::fetched_files_or_skip() {
        let bytes = std::fs::read(&path).unwrap();
        let d = Drawing::from_bytes(&bytes).unwrap();
        if d.source_version() == Some(opendwg::Version::R2000) {
            continue;
        }
        let cut = Drawing::from_bytes(&bytes[..bytes.len() - 1])
            .unwrap_or_else(|err| panic!("{}: {err}", e.name));
        assert!(
            cut.diagnostics()
                .iter()
                .any(|x| x.message.contains("every section was read")),
            "{}: {:?}",
            e.name,
            cut.diagnostics()
        );
        assert_eq!(cut.objects().count(), d.objects().count(), "{}", e.name);
        done += 1;
    }
    if done > 0 {
        assert!(done >= 5, "{done}");
    }
}
