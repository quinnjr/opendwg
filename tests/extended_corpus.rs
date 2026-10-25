//! The extended public corpus (`corpus/extended.toml`): real-world drawings
//! from AutoCAD, Civil 3D, BricsCAD, ODA/Teigha, SolidWorks and other
//! writers, and files the crate does not read.
//!
//! Every file a supported version: opens without a panic or an error
//! diagnostic, validates without an error, and either saves, reads back
//! and validates clean (the objects as read, but those the writer leaves out
//! or adds, which a second save keeps), or holds objects R2018 cannot store
//! and is refused with [`Error::UnconvertibleObjects`]. Every other file is
//! refused with [`Error::UnsupportedVersion`]. Then feature tests: xrefs,
//! GBK text, and Civil 3D objects kept byte for byte.
//!
//! The files are fetched by `scripts/fetch-corpus.sh`; a missing one is
//! skipped (a failure with `OPENDWG_REQUIRE_CORPUS=1`).

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use opendwg::object::{Entity, Object};
use opendwg::{Drawing, Error, Handle, Severity};

/// Research ids left out: third-party content under a repository licence
/// (Mapwize, Autodesk samples redistributed by Aspose, a user's file, an
/// architectural plan in a site template's repository).
const EXCLUDED: [&str; 7] = [
    "gdal-01", "asp-01", "asp-03", "asp-13", "asp-16", "ezdxf-05", "bur-01",
];

/// Object types the writer leaves out (see `drawing::write`): the R2000
/// VP_ENT_HDR table and a pre-R2010 CELLSTYLEMAP its converted table style
/// absorbs.
const LEFT_OUT: [&str; 3] = ["VP_ENT_HDR", "VP_ENT_HDR_CONTROL", "CELLSTYLEMAP"];

fn expect_str<'a>(e: &'a common::Entry, key: &str) -> Option<&'a str> {
    e.expect.get(key).and_then(toml::Value::as_str)
}

fn expect_bool(e: &common::Entry, key: &str) -> bool {
    e.expect
        .get(key)
        .and_then(toml::Value::as_bool)
        .unwrap_or(false)
}

fn open(name: &str) -> Option<Drawing> {
    let p = common::corpus_file_or_skip(name)?;
    Some(Drawing::open(p).unwrap_or_else(|e| panic!("{name}: {e}")))
}

#[test]
fn extended_manifest_is_well_formed() {
    let m = common::load_extended();
    if m.file.is_empty() {
        return;
    }
    let main: BTreeSet<String> = common::load_manifest()
        .file
        .into_iter()
        .map(|e| e.name)
        .collect();
    let mut names = BTreeSet::new();
    for e in &m.file {
        let n = &e.name;
        assert!(names.insert(n.clone()), "duplicate name {n}");
        assert!(!main.contains(n), "{n} is in manifest.toml too");
        assert!(!EXCLUDED.contains(&n.as_str()), "{n} is excluded");
        assert!(e.url.starts_with("https://"), "{n}: url must be https");
        let pinned = e
            .url
            .split('/')
            .any(|seg| seg.len() == 40 && seg.bytes().all(|b| b.is_ascii_hexdigit()));
        // US federal files have no commit; the sha256 pins them.
        assert!(
            pinned || e.url.starts_with("https://www.fs.usda.gov/"),
            "{n}: url must be pinned to a 40-hex commit SHA"
        );
        assert_eq!(e.sha256.len(), 64, "{n}: bad sha256");
        assert!(e.sha256.bytes().all(|b| b.is_ascii_hexdigit()), "{n}");
        assert!(!e.license_note.is_empty(), "{n}: empty license_note");
        assert!(e.producer.as_deref().is_some_and(|p| !p.is_empty()), "{n}");
        for f in &e.features {
            assert!(common::FEATURES.contains(&f.as_str()), "{n}: feature {f}");
        }
        if let Some(open) = expect_str(e, "open") {
            assert_eq!(open, "unsupported", "{n}");
        }
    }
}

/// What a file did, or the first thing that went wrong.
fn check(e: &common::Entry, path: &PathBuf) -> Result<String, String> {
    let name = &e.name;
    let bytes = std::fs::read(path).map_err(|x| x.to_string())?;
    if bytes.get(..6) != Some(e.version.as_bytes()) {
        return Err(format!("magic {:?} is not {:?}", bytes.get(..6), e.version));
    }
    let opened = std::panic::catch_unwind(|| Drawing::from_bytes(&bytes))
        .map_err(|_| "panic while opening".to_string())?;
    if expect_str(e, "open") == Some("unsupported") {
        return match opened {
            Err(Error::UnsupportedVersion(_)) => Ok("unsupported".into()),
            Err(x) => Err(format!("not UnsupportedVersion: {x}")),
            Ok(_) => Err("opened".into()),
        };
    }
    let d = opened.map_err(|x| format!("open: {x}"))?;
    let errors = |v: &[opendwg::Diagnostic]| -> Vec<String> {
        v.iter()
            .filter(|x| x.severity == Severity::Error)
            .take(3)
            .map(ToString::to_string)
            .collect()
    };
    let read = errors(d.diagnostics());
    if !read.is_empty() {
        return Err(format!("read errors: {read:?}"));
    }
    let validated = d.validate();
    let invalid = errors(&validated);
    if !invalid.is_empty() {
        return Err(format!("validate errors: {invalid:?}"));
    }
    let unconvertible = !d.unconvertible_objects().is_empty();
    if unconvertible != expect_bool(e, "unconvertible") {
        return Err(format!(
            "unconvertible objects {:?}, manifest says {}",
            d.unconvertible_objects().first(),
            expect_bool(e, "unconvertible")
        ));
    }
    if unconvertible {
        return match d.to_bytes() {
            Err(Error::UnconvertibleObjects(_)) => Ok("unconvertible".into()),
            Err(x) => Err(format!("save: {x}")),
            Ok(_) => Err("saved with unconvertible objects".into()),
        };
    }
    let (saved, warnings) = d
        .to_bytes_with_diagnostics()
        .map_err(|x| format!("save: {x}"))?;
    // The dry run lists what the write reported.
    if warnings != d.write_diagnostics() {
        return Err("write_diagnostics differs from the write's warnings".into());
    }
    let back = Drawing::from_bytes(&saved).map_err(|x| format!("reopen: {x}"))?;
    let invalid = errors(&back.validate());
    if !invalid.is_empty() {
        return Err(format!("validate after the round trip: {invalid:?}"));
    }
    // The objects as read, but those the writer leaves out, and those it
    // adds (one warning each from validate), which are new handles.
    let before: BTreeSet<Handle> = d.objects().map(|(h, _)| h).collect();
    let after: BTreeSet<Handle> = back.objects().map(|(h, _)| h).collect();
    for h in before.difference(&after) {
        let t = d.get(*h).unwrap().type_name();
        if !LEFT_OUT.contains(&t.as_ref()) {
            return Err(format!("{t} {h} lost in the round trip"));
        }
    }
    let added = after.difference(&before).count();
    let repairs = validated
        .iter()
        .filter(|x| x.message.contains("the writer adds"))
        .count();
    if added != repairs {
        return Err(format!(
            "{added} objects added, {repairs} repairs announced"
        ));
    }
    // A second save keeps every object.
    let again = Drawing::from_bytes(&back.to_bytes().map_err(|x| format!("save 2: {x}"))?)
        .map_err(|x| format!("reopen 2: {x}"))?;
    if again.objects().count() != back.objects().count() {
        return Err("the second round trip changed the object count".into());
    }
    let _ = name;
    Ok(format!("round trip, {} objects", back.objects().count()))
}

/// Every extended corpus file, on a few threads; a debug build skips files
/// over 2 MB (run the corpus tests with `--release`).
#[test]
fn every_extended_file_opens_validates_and_saves() {
    let files: Vec<(common::Entry, PathBuf)> = common::load_extended()
        .file
        .into_iter()
        .filter_map(|e| common::corpus_file_or_skip(&e.name).map(|p| (e, p)))
        .filter(|(e, p)| {
            let big = std::fs::metadata(p).map_or(0, |m| m.len()) > 2_000_000;
            if big && cfg!(debug_assertions) {
                eprintln!("SKIPPED in a debug build: {} (over 2 MB)", e.name);
            }
            !(big && cfg!(debug_assertions))
        })
        .collect();
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get().min(8));
    let chunk = files.len().div_ceil(threads).max(1);
    let results: BTreeMap<String, Result<String, String>> = std::thread::scope(|s| {
        let handles: Vec<_> = files
            .chunks(chunk)
            .map(|part| {
                s.spawn(move || {
                    part.iter()
                        .map(|(e, p)| (e.name.clone(), check(e, p)))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().unwrap())
            .collect()
    });
    let failed: Vec<String> = results
        .iter()
        .filter_map(|(n, r)| r.as_ref().err().map(|e| format!("{n}: {e}")))
        .collect();
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}

/// The block record named `name` of `d`.
fn block<'a>(d: &'a Drawing, name: &str) -> &'a opendwg::object::BlockRecord {
    d.block_records()
        .map(|(_, b)| b)
        .find(|b| b.entry.name == name)
        .unwrap_or_else(|| panic!("no block {name}"))
}

/// civilcraft's sheet set: the R2000 `Details.dwg` holds the unloaded xref
/// block "Level 1" pointing at `..\plans\Level 1.dwg` (cc-13).
#[test]
fn xref_block_of_an_r2000_host() {
    let Some(d) = open("cc-12") else { return };
    let b = block(&d, "Level 1");
    assert!(b.is_xref);
    assert_eq!(b.xref_path, r"..\plans\Level 1.dwg");
}

/// Civil 3D's xref host `XrefTests.dwg` attaches `Xref01.dwg` (camber-24):
/// the xref block record decodes (R2004+ xref blocks store no owned count)
/// and its dependent layers are named `Xref01|...`.
#[test]
fn xref_block_of_a_civil_3d_host() {
    let Some(d) = open("camber-26") else { return };
    let b = block(&d, "Xref01");
    assert!(b.is_xref);
    assert!(b.xref_path.ends_with("Xref01.dwg"), "{:?}", b.xref_path);
    assert!(
        d.layers()
            .any(|(_, l)| l.entry.name.starts_with("Xref01|") && l.entry.xref_dependent)
    );
    assert!(
        d.diagnostics()
            .iter()
            .all(|x| !x.message.contains("BLOCK_HEADER"))
    );
}

/// GBK (ANSI_936) text decodes to Chinese: R2000 files from SolidWorks and a
/// Chinese AutoCAD, R2013 files (UTF-16 text, GBK code page).
#[test]
fn gbk_text_decodes() {
    if let Some(d) = open("dsh-01") {
        assert_eq!(d.header.dwgcodepage, 39);
        assert!(d.layers().any(|(_, l)| l.entry.name == "轮廓实线层"));
    }
    for name in ["wz-01", "wz-02", "fox-01"] {
        if let Some(d) = open(name) {
            assert_eq!(d.header.dwgcodepage, 39, "{name}");
            assert!(d.layouts().iter().any(|(_, l)| l.name == "布局1"), "{name}");
        }
    }
    if let Some(d) = open("wz-02") {
        assert!(d.objects().any(|(_, o)| matches!(o,
            Object::Entity { entity: Entity::AttDef(a), .. } if a.prompt == "输入电气设备名称:")));
    }
    if let Some(d) = open("fox-02") {
        assert!(
            d.objects().any(|(_, o)| matches!(o,
            Object::Entity { entity: Entity::Text(t), .. } if t.value == "景观绿地-景观-景观线稿"))
        );
    }
}

/// Civil 3D's AECC objects (classes the library does not model) are kept
/// raw and written back byte for byte, as are all other objects of these
/// AutoCAD-written files.
#[test]
fn civil_3d_objects_round_trip_byte_identical() {
    for name in ["camber-01", "camber-18", "camber-26", "c3dtk-01", "rc-03"] {
        let Some(p) = common::corpus_file_or_skip(name) else {
            continue;
        };
        let bytes = std::fs::read(p).unwrap();
        let d = Drawing::from_bytes(&bytes).unwrap();
        let out = d.to_bytes().unwrap();
        let (before, after) = (common::object_bytes(&bytes), common::object_bytes(&out));
        let mut aecc = 0;
        for (h, o) in d.objects() {
            if let Some(raw) = o.raw() {
                if raw
                    .class_name
                    .as_deref()
                    .is_some_and(|c| c.starts_with("AECC_"))
                {
                    aecc += 1;
                }
            }
            assert_eq!(
                before.get(&h),
                after.get(&h),
                "{name}: {} {h}",
                o.type_name()
            );
        }
        assert!(aecc > 100, "{name}: {aecc} AECC objects");
        let back = Drawing::from_bytes(&out).unwrap();
        assert!(back.objects().filter(|(_, o)| o.raw().is_some()).count() >= aecc);
    }
}

/// Ownership AutoCAD and Civil 3D write but the library cannot judge is a
/// warning, not a refusal; required objects other writers leave out are
/// added at save.
#[test]
fn relaxed_checks_warn() {
    let warned = |name: &str, what: &str| {
        let Some(d) = open(name) else { return };
        let v = d.validate();
        assert!(v.iter().all(|x| x.severity != Severity::Error), "{name}");
        assert!(
            v.iter().any(|x| x.message.contains(what)),
            "{name}: no warning with {what:?}"
        );
    };
    warned("camber-01", "AECC_LABEL_TEXT_STYLE");
    warned("adsk-py-01", "does not list it");
    warned("adsk-cab-04", "names an object that does not exist");
    warned("brx-01", "dimension style \"Standard\"");
    warned("cc-12", "ACAD_PLOTSETTINGS");
    // Several "*Active" VPORTs: no finding at all.
    if let Some(d) = open("rc-02") {
        assert!(
            d.validate()
                .iter()
                .all(|x| !x.message.contains("same name"))
        );
    }
    // A null owner of a listed object is set by the writer, with a warning.
    if let Some(d) = open("arx-02") {
        assert!(
            d.write_diagnostics()
                .iter()
                .any(|x| x.message.contains("owner is null"))
        );
    }
}
