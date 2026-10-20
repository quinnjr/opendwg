//! Proxy graphics on the sample corpus (ODS chapter 29).

mod common;

use std::collections::BTreeMap;

use opendwg::container;
use opendwg::object::{EntityCommon, Object};
use opendwg::proxy_graphics::{self, ProxyPrimitive as P};
use opendwg::{Drawing, Handle, Version};

/// Opcodes the corpus has that ODS does not describe (see the module
/// documentation of `proxy_graphics`): 21 is empty and 51 holds one `RL`.
const OPCODES_NOT_IN_ODS: [u32; 2] = [21, 51];

/// Every entity of a drawing with proxy graphics: handle, type name, common data.
fn proxy_entities(d: &Drawing) -> Vec<(Handle, String, &EntityCommon)> {
    d.objects()
        .filter_map(|(h, o)| match o {
            Object::Entity { common, .. } if common.proxy_graphics.is_some() => {
                Some((h, o.type_name().into_owned(), common))
            }
            _ => None,
        })
        .collect()
}

/// The records of a blob: opcode and data length.
fn records(blob: &[u8]) -> Vec<(u32, usize)> {
    let rl = |at: usize| u32::from_le_bytes(blob[at..at + 4].try_into().unwrap());
    let mut at = 8;
    let mut out = vec![];
    while at < blob.len() {
        out.push((rl(at + 4), rl(at) as usize - 8));
        at += rl(at) as usize;
    }
    out
}

/// The data length of an opcode whose layout is fixed, given a record's
/// `len` for the ones with a point count.
fn exact_len(op: u32, len: usize) -> Option<usize> {
    Some(match op {
        14 | 16 | 18 | 19 | 20 | 22 | 23 => 4,
        24 | 25 | 26 | 34 => 8,
        28 | 31 => 0,
        2 => 56,
        29 => 128,
        6 | 7 => 4 + 24 * ((len.checked_sub(4)?) / 24),
        32 => 4 + 24 * ((len.checked_sub(28)?) / 24) + 24,
        _ => return None,
    })
}

/// A blob of the records, with sizes consistent.
fn rebuild(records: &[(u32, &[u8])]) -> Vec<u8> {
    let body: Vec<u8> = records
        .iter()
        .flat_map(|(op, d)| {
            let mut v = (d.len() as u32 + 8).to_le_bytes().to_vec();
            v.extend(op.to_le_bytes());
            v.extend_from_slice(d);
            v
        })
        .collect();
    let mut v = (body.len() as u32 + 8).to_le_bytes().to_vec();
    v.extend((records.len() as u32).to_le_bytes());
    v.extend(body);
    v
}

fn is_geometric(p: &P) -> bool {
    matches!(
        p,
        P::Extents(..)
            | P::Circle { .. }
            | P::Circle3Pt(_)
            | P::CircularArc { .. }
            | P::CircularArc3Pt { .. }
            | P::Polyline(_)
            | P::Polygon(_)
            | P::PolylineWithNormal { .. }
            | P::Mesh { .. }
            | P::Shell { .. }
            | P::Text { .. }
            | P::Text2(_)
            | P::Xline(..)
            | P::Ray(..)
            | P::LwPolyline(_)
    )
}

#[test]
fn proxy_decode_corpus() {
    let mut files_with = vec![];
    for (e, path) in common::fetched_files_or_skip() {
        let bytes = std::fs::read(&path).unwrap();
        let raw = container::read(&bytes).unwrap();
        let d = Drawing::from_bytes(&bytes).unwrap();
        let entities = proxy_entities(&d);
        let tagged = e.features.iter().any(|f| f == "proxy_graphics");
        assert_eq!(
            tagged,
            !entities.is_empty(),
            "{}: the proxy_graphics feature tag must match the file",
            e.name
        );
        if entities.is_empty() {
            continue;
        }
        files_with.push((e.name.clone(), entities.len()));
        let mut geometric = 0;
        for (h, ty, common) in entities {
            let blob = common.proxy_graphics.as_ref().unwrap();
            // The size read from the entity (an RL before R2010, a BLL from
            // R2010) is the blob's own total size.
            let total = u32::from_le_bytes(blob[..4].try_into().unwrap()) as usize;
            assert_eq!(total, blob.len(), "{}: {h} {ty}: size mismatch", e.name);
            let prims = common
                .proxy_primitives_with_codepage(raw.version, raw.codepage)
                .unwrap()
                .unwrap_or_else(|err| panic!("{}: {h} {ty}: {err}", e.name));
            geometric += prims.iter().filter(|p| is_geometric(p)).count();
            for p in &prims {
                match p {
                    P::Unknown { opcode, .. } => assert!(
                        OPCODES_NOT_IN_ODS.contains(opcode),
                        "{}: {h} {ty}: unknown opcode {opcode}",
                        e.name
                    ),
                    P::Undecoded { opcode, error, .. } => {
                        panic!("{}: {h} {ty}: opcode {opcode} undecoded: {error}", e.name)
                    }
                    _ => {}
                }
            }
            // Records whose layout has no trailing data are consumed to
            // within their padding.
            for (op, len) in records(blob) {
                if let Some(want) = exact_len(op, len) {
                    assert_eq!(len, want, "{}: {h} {ty}: opcode {op}", e.name);
                }
            }
        }
        assert!(geometric > 0, "{}: no geometric primitive", e.name);
    }
    eprintln!("proxy graphics per file: {files_with:?}");
}

/// Primitives decode to the values the samples' drawings contain.
#[test]
fn proxy_corpus_values() {
    // R2000: code page text (opcode 11); R2018: Unicode text (opcode 38).
    for (name, version) in [
        ("acadsharp-sample-ac1015", Version::R2000),
        ("acadsharp-sample-ac1018", Version::R2004),
        ("acadsharp-sample-ac1021", Version::R2007),
        ("acadsharp-sample-ac1032", Version::R2018),
    ] {
        let Some(path) = common::corpus_file_or_skip(name) else {
            continue;
        };
        let d = Drawing::from_bytes(&std::fs::read(path).unwrap()).unwrap();
        let mut texts = vec![];
        let mut shells = 0;
        for (_, _, c) in proxy_entities(&d) {
            for p in c.proxy_primitives(version).unwrap().unwrap() {
                match p {
                    P::Text2(t) => {
                        assert_eq!(t.font.is_some(), version >= Version::R2007, "{name}");
                        texts.push(t.text);
                    }
                    P::Shell { faces, .. } => {
                        shells += 1;
                        assert_eq!(faces, [[0, 1, 2, 3]], "{name}");
                    }
                    _ => {}
                }
            }
        }
        assert!(
            texts.contains(&"Table sample".to_owned()),
            "{name}: {texts:?}"
        );
        assert!(
            texts.contains(&"Hello this is a title".to_owned()),
            "{name}"
        );
        assert_eq!(shells, 1, "{name}");
    }
    // AEC entities use the Unicode text of opcode 36.
    if let Some(path) = common::corpus_file_or_skip("acadsharp-aec-objects") {
        let d = Drawing::from_bytes(&std::fs::read(path).unwrap()).unwrap();
        let mut found: BTreeMap<String, String> = BTreeMap::new();
        for (_, ty, c) in proxy_entities(&d) {
            for p in c.proxy_primitives(Version::R2018).unwrap().unwrap() {
                if let P::Text { text, height, .. } = p {
                    assert_eq!(height, 2.5);
                    found.insert(ty.clone(), text);
                }
            }
        }
        assert_eq!(
            found.get("AEC_WALL").map(String::as_str),
            Some("AecDbWall (AecArchBase80)")
        );
        assert_eq!(found.len(), 3, "{found:?}");
    }
}

/// A blob cut at any byte boundary is an error, never a panic.
#[test]
fn proxy_truncated_errors() {
    for (e, path) in common::fetched_files_or_skip() {
        let d = Drawing::from_bytes(&std::fs::read(path).unwrap()).unwrap();
        let version = Version::from_magic(e.version.as_bytes().try_into().unwrap()).unwrap();
        // The longest and the shortest blob keep this quick on big drawings.
        let mut blobs: Vec<&Vec<u8>> = proxy_entities(&d)
            .into_iter()
            .filter_map(|(_, _, c)| c.proxy_graphics.as_ref())
            .collect();
        blobs.sort_by_key(|b| b.len());
        let Some(first) = blobs.first() else { continue };
        for blob in [first, blobs.last().unwrap()] {
            for n in 0..blob.len() {
                assert!(
                    proxy_graphics::decode(&blob[..n], version).is_err(),
                    "{}: cut at {n} of {}",
                    e.name,
                    blob.len()
                );
                // With the total size patched, the record loop reaches the cut.
                if n >= 8 {
                    let mut cut = blob[..n].to_vec();
                    cut[..4].copy_from_slice(&(n as u32).to_le_bytes());
                    assert!(
                        proxy_graphics::decode(&cut, version).is_err(),
                        "{}: patched cut at {n} of {}",
                        e.name,
                        blob.len()
                    );
                }
            }
        }
        // Each distinct record cut short inside a consistent blob never
        // panics; where the layout is fixed, it is undecoded.
        let mut seen = std::collections::BTreeSet::new();
        for blob in &blobs {
            let mut at = 8;
            for (op, len) in records(blob) {
                let data = &blob[at + 8..at + 8 + len];
                at += 8 + len;
                if !seen.insert((op, len)) {
                    continue;
                }
                for k in 0..len {
                    let out = proxy_graphics::decode(&rebuild(&[(op, &data[..k])]), version)
                        .unwrap_or_else(|err| panic!("{}: opcode {op} cut at {k}: {err}", e.name));
                    assert_eq!(out.len(), 1);
                    if exact_len(op, len).is_some() {
                        assert!(
                            matches!(out[0], P::Undecoded { .. }),
                            "{}: opcode {op} cut at {k} of {len}",
                            e.name
                        );
                    }
                }
            }
        }
    }
}
