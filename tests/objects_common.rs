//! Corpus check of the object streams and common data (Task 10): every object
//! in every fetched sample splits with a valid CRC, every fixed-type object
//! and entity decodes its common data, and the decoded data re-encodes to
//! R2018 and decodes back equal. Layer and linetype references must resolve
//! to LAYER and LTYPE objects, and every R2007+ string stream must be exactly
//! a run of `TU` strings, which pins the string stream position and size
//! fields (including the 0x8000 size extension) against real files.

mod common;

use std::collections::HashMap;

use opendwg::Version;
use opendwg::container::{self, SectionName};
use opendwg::object::{
    LinetypeRef, ObjReader, ObjWriter, decode_entity_common, decode_object_common,
    encode_entity_common, encode_object_common,
};
use opendwg::section::objmap;

const LAYER: u16 = 0x33;
const LTYPE: u16 = 0x39;

/// Fixed entity type codes (ODS 20.3).
fn is_entity(t: u16) -> bool {
    matches!(t, 1..=8 | 0xA..=0x29 | 0x2C..=0x2F | 0x4D | 0x4E)
}

/// Fixed non-entity type codes whose layout starts with the common data.
fn is_object(t: u16) -> bool {
    matches!(t, 0x2A | 0x30..=0x35 | 0x38 | 0x39 | 0x3C..=0x49 | 0x4F | 0x52)
}

#[test]
fn corpus_objects_split_decode_and_reencode() {
    let files = common::fetched_files_or_skip();
    let mut entities = 0usize;
    let mut big_string_streams = 0usize;
    let mut saw_r2007_plus = false;
    for (entry, path) in files {
        let name = &entry.name;
        let bytes = std::fs::read(&path).unwrap();
        let raw = container::read(&bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
        let v = raw.version;
        saw_r2007_plus |= v >= Version::R2007;
        let objects = &raw.sections[&SectionName::Objects];
        let map: Vec<(u64, u64)> = objmap::decode(&raw.sections[&SectionName::Handles])
            .unwrap_or_else(|e| panic!("{name}: object map: {e}"))
            .into_iter()
            .map(|(h, loc)| (h.0, loc))
            .collect();
        assert!(!map.is_empty(), "{name}: empty object map");

        let mut types = HashMap::new();
        for &(h, loc) in &map {
            let slice = objects
                .get(loc as usize..)
                .unwrap_or_else(|| panic!("{name}: {h:X} location out of range"));
            let (r, pre) = ObjReader::split(slice, v, raw.codepage)
                .unwrap_or_else(|e| panic!("{name}: split {h:X}: {e}"));
            assert!(r.crc_ok(), "{name}: CRC mismatch in {h:X}");
            types.insert(h, pre.type_code);
            if let Some(s) = &r.strings {
                let mut s = s.clone();
                big_string_streams += usize::from(s.remaining_bits() >= 0x8000);
                while s.remaining_bits() > 0 {
                    let text = s
                        .read_tu()
                        .unwrap_or_else(|e| panic!("{name}: string stream of {h:X}: {e}"));
                    assert!(!text.ends_with('\0'), "{name}: {h:X}: NUL-terminated TU");
                }
            }
        }

        for &(h, loc) in &map {
            let (mut r, pre) = ObjReader::split(&objects[loc as usize..], v, raw.codepage).unwrap();
            let t = pre.type_code;
            if is_entity(t) {
                let e = decode_entity_common(&mut r)
                    .unwrap_or_else(|err| panic!("{name}: entity {h:X} type {t:X}: {err}"));
                entities += 1;
                assert_eq!(e.obj.handle.0, h, "{name}: handle of {h:X}");
                assert_eq!(
                    types.get(&e.layer.0),
                    Some(&LAYER),
                    "{name}: layer of {h:X}"
                );
                if let LinetypeRef::Handle(lt) = e.linetype {
                    assert_eq!(types.get(&lt.0), Some(&LTYPE), "{name}: linetype of {h:X}");
                }
                assert!(
                    r.diagnostics.is_empty(),
                    "{name}: {h:X}: {:?}",
                    r.diagnostics
                );
                let mut w = ObjWriter::new(e.obj.handle);
                encode_entity_common(&mut w, &e).unwrap();
                let out = w.finish(t).unwrap();
                let (mut r2, _) = ObjReader::split(&out, Version::R2018, 30).unwrap();
                let mut back = decode_entity_common(&mut r2).unwrap();
                // R2000 links are not written (R2018 has none).
                back.prev_next = e.prev_next;
                assert_eq!(back, e, "{name}: re-encode of entity {h:X}");
            } else if is_object(t) {
                let c = decode_object_common(&mut r)
                    .unwrap_or_else(|err| panic!("{name}: object {h:X} type {t:X}: {err}"));
                assert_eq!(c.handle.0, h, "{name}: handle of {h:X}");
                let mut w = ObjWriter::new(c.handle);
                encode_object_common(&mut w, &c).unwrap();
                let out = w.finish(t).unwrap();
                let (mut r2, _) = ObjReader::split(&out, Version::R2018, 30).unwrap();
                assert_eq!(
                    decode_object_common(&mut r2).unwrap(),
                    c,
                    "{name}: re-encode of object {h:X}"
                );
            }
        }
    }
    if entities > 0 {
        assert!(entities > 1000, "only {entities} entities decoded");
    }
    if saw_r2007_plus {
        assert!(
            big_string_streams > 0,
            "no string stream used the 0x8000 size extension"
        );
    }
}
