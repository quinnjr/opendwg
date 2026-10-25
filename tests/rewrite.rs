//! Rewriting a whole drawing's handles. Every corpus file that saves
//! as read is renumbered by +0x1000, saved and read back: the copy validates
//! without errors, and renumbered back by -0x1000 it has the model of the
//! file saved without renumbering.

mod common;

use std::collections::BTreeMap;

use opendwg::object::{AcisGuid, AcisPayload, Entity, Object};
use opendwg::section::JulianDate;
use opendwg::{Drawing, Handle, Severity};

const SAVED: JulianDate = JulianDate {
    day: 2_461_300,
    ms: 43_200_000,
};

const OFFSET: u64 = 0x1000;

fn read_back(d: &Drawing, what: &str) -> Drawing {
    let bytes = d
        .to_bytes_at(SAVED)
        .unwrap_or_else(|e| panic!("{what}: {e:?}"));
    let back = Drawing::from_bytes(&bytes).unwrap_or_else(|e| panic!("{what}: {e}"));
    assert!(
        back.diagnostics().is_empty(),
        "{what}: {:?}",
        &back.diagnostics()[..back.diagnostics().len().min(5)]
    );
    back
}

fn acis(d: &Drawing) -> BTreeMap<Handle, Option<Vec<u8>>> {
    d.objects()
        .filter(|(_, o)| {
            matches!(
                o,
                Object::Entity {
                    entity: Entity::Acis(_),
                    ..
                }
            )
        })
        .map(|(h, _)| (h, d.acis_data(h).map(<[u8]>::to_vec)))
        .collect()
}

/// Whether `o` is an ACIS entity the writer converts to the R2013+ form at
/// save (read from R2004-R2010): it then gets a GUID derived from its handle
/// (see `AcisGuid::derived`), so one saved under another handle has another
/// GUID.
fn converted_at_save(o: &Object) -> bool {
    matches!(
        o,
        Object::Entity {
            entity: Entity::Acis(a),
            ..
        } if !matches!(a.payload, AcisPayload::DataStorage { .. })
    )
}

/// `o` with its ACIS GUID cleared.
fn without_guid(o: &Object) -> Object {
    let mut o = o.clone();
    if let Object::Entity {
        entity: Entity::Acis(a),
        ..
    } = &mut o
    {
        if let AcisPayload::DataStorage { guid, .. } = &mut a.payload {
            *guid = AcisGuid::default();
        }
    }
    o
}

#[test]
fn renumbered_corpus_files_save_and_read_back() {
    let mut checked = 0;
    for (e, p) in common::fetched_files_or_skip() {
        let what = e.name.as_str();
        let d = Drawing::open(&p).unwrap();
        if !d.unconvertible_objects().is_empty() {
            continue;
        }
        let plain = read_back(&d, what);

        let mut moved = d.clone();
        moved.rewrite_handles(|h| Handle(h.0 + OFFSET)).unwrap();
        let largest = moved.objects().map(|(h, _)| h).max().unwrap();
        assert!(moved.header.handseed > largest, "{what}");
        let back = read_back(&moved, what);
        let errors: Vec<_> = back
            .validate()
            .into_iter()
            .filter(|x| x.severity == Severity::Error)
            .collect();
        assert!(errors.is_empty(), "{what}: {errors:?}");
        // The objects the plain save writes (it may leave some out), moved.
        let keys = |d: &Drawing, by: u64| -> Vec<Handle> {
            d.objects().map(|(h, _)| Handle(h.0 + by)).collect()
        };
        assert!(keys(&back, 0) == keys(&plain, OFFSET), "{what}: handles");

        // Moved back, it is the drawing saved without moving.
        let mut again = back.clone();
        again.rewrite_handles(|h| Handle(h.0 - OFFSET)).unwrap();
        let mut header = again.header.clone();
        header.handseed = plain.header.handseed;
        assert_eq!(header, plain.header, "{what}: header");
        assert_eq!(again.classes, plain.classes, "{what}: classes");
        let mut differ = Vec::new();
        for (h, o) in plain.objects() {
            let Some(b) = again.get(h) else {
                differ.push(format!("{h} {} missing", o.type_name()));
                continue;
            };
            // Only an entity converted at save may differ, in its GUID.
            let converted = d.get(h).is_some_and(converted_at_save);
            let same = if converted {
                without_guid(b) == without_guid(o)
            } else {
                b == o
            };
            if !same {
                differ.push(format!("{h} {}", o.type_name()));
            }
        }
        assert!(differ.is_empty(), "{what}: {differ:?}");
        assert_eq!(again.objects().count(), plain.objects().count(), "{what}");
        assert_eq!(acis(&again), acis(&plain), "{what}: ACIS data");
        checked += 1;
    }
    if checked > 0 {
        assert!(checked >= 4, "{checked}");
    }
}

/// The decimal numbers right after each `prefix` in `text`.
fn numbers_after(text: &str, prefix: &str) -> Vec<u64> {
    text.match_indices(prefix)
        .filter_map(|(at, _)| {
            let rest = &text[at + prefix.len()..];
            let end = rest
                .find(|c: char| !c.is_ascii_digit())
                .unwrap_or(rest.len());
            rest[..end].parse().ok()
        })
        .collect()
}

/// The (code, value) of each `HandleRef` in a debug form.
fn href_fields(text: &str) -> Vec<(u64, u64)> {
    text.match_indices("HandleRef { code: ")
        .map(|(at, p)| {
            let rest = &text[at + p.len()..];
            let code = numbers_after(rest, "")[0];
            let value = numbers_after(rest, ", value: ")[0];
            (code, value)
        })
        .collect()
}

/// `rewrite_reaches_every_handle` (src/object/remap.rs) on drawings built
/// through the API: a new drawing, and one with every entity kind and the
/// objects they use. After renumbering by +0x1000 no object's debug form
/// holds a handle or stored reference below it (null aside), and no
/// relative reference code (above 5) is left.
#[test]
fn rewrite_reaches_every_handle_of_built_drawings() {
    let (every, _) = common::every_entity::with_every_entity(Drawing::new());
    for (name, d) in [("new", Drawing::new()), ("every entity", every)] {
        let mut d = d;
        d.rewrite_handles(|h| Handle(h.0 + OFFSET)).unwrap();
        let mut checked = 0;
        for (h, obj) in d.objects() {
            let text = format!("{obj:?}");
            for v in numbers_after(&text, "Handle(") {
                assert!(v == 0 || v >= OFFSET, "{name} {h}: Handle({v}) in {text}");
            }
            for (code, value) in href_fields(&text) {
                assert!(code <= 5, "{name} {h}: code {code}");
                assert!(value == 0 || value >= OFFSET, "{name} {h}: value {value}");
            }
            checked += 1;
        }
        assert!(checked > 100, "{name}: {checked}");
        read_back(&d, name);
    }
}
