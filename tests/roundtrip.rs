//! Task 22: writing drawings as R2018. Every corpus file is read, written
//! with [`Drawing::to_bytes_at`] and read back: the second read has no
//! diagnostics and the same model as the first (the save time, the handle
//! seed and the application information aside). Files with objects R2018
//! cannot hold refuse to save, listing exactly those objects; once they are
//! removed through the API the rest of the drawing saves.

mod common;

use common::remove::remove_all;

use std::collections::{BTreeMap, HashSet};
use std::time::Instant;

use opendwg::container::{self, SectionName};
use opendwg::object::{
    AcisGuid, DecodeCtx, Entity, EntityCommon, EntityList, Line, NonGraphical, Object, PolygonMesh,
    Vertex3d, decode_object, encode_object,
};
use opendwg::section::{AppInfo, JulianDate};
use opendwg::{Drawing, Error, Handle, Severity, Version};

const R2018: &str = "acadsharp-sample-ac1032";

/// The save time used throughout, so the bytes are reproducible.
const SAVED: JulianDate = JulianDate {
    day: 2_461_300,
    ms: 43_200_000,
};

fn open(name: &str) -> Option<Drawing> {
    let p = common::corpus_file_or_skip(name)?;
    Some(Drawing::open(p).unwrap())
}

/// Write `d` and read it back; the read must have no diagnostics.
fn roundtrip(d: &Drawing, what: &str) -> Drawing {
    let bytes = d
        .to_bytes_at(SAVED)
        .unwrap_or_else(|e| panic!("{what}: {e:?}"));
    let back = Drawing::from_bytes(&bytes).unwrap_or_else(|e| panic!("{what}: re-read: {e}"));
    assert!(
        back.diagnostics().is_empty(),
        "{what}: {:?}",
        &back.diagnostics()[..back.diagnostics().len().min(10)]
    );
    back
}

/// `o` with its raw bits marked as R2018 ones: a raw object read from an
/// R2007+ file is written unchanged, and reads back as R2018 bits.
fn normalized(o: &Object) -> Object {
    let mut o = o.clone();
    if let Object::Entity {
        entity: Entity::Unknown(r),
        ..
    }
    | Object::Other {
        object: NonGraphical::Unknown(r),
        ..
    } = &mut o
    {
        r.source_version = Version::R2018;
    }
    o
}

/// Object `h` of `d` as an R2018 read gives it back: encoded and decoded
/// alone. Converting an object from an older version (fields R2018 does not
/// store, a pre-R2010 table style, raw bit padding) is the object encoders'
/// part, tested with them; this leaves the drawing's part to compare.
fn as_r2018(d: &Drawing, h: Handle, o: &Object) -> Object {
    // An ACIS entity read before R2013 is converted by the drawing writer;
    // one with data gets its data storage record, and the flag saying so.
    let mut o = o.clone();
    if let Object::Entity {
        common,
        entity: Entity::Acis(a),
    } = &mut o
    {
        let guid = AcisGuid::derived(&d.header.fingerprintguid, h);
        let (converted, data) = a.to_r2013(guid).unwrap();
        **a = converted;
        if data.is_some() {
            common.obj.has_ds_binary_data = true;
        }
    }
    let bytes = encode_object(&o, &d.classes).unwrap();
    let mut ctx = DecodeCtx::new(Version::R2018, d.header.dwgcodepage, &d.classes);
    let back = decode_object(&bytes, h, &mut ctx).unwrap();
    assert!(ctx.diagnostics.is_empty(), "{h}: {:?}", ctx.diagnostics);
    back
}

/// Where the debug forms of `a` and `b` first differ.
fn debug_diff(a: &Object, b: &Object) -> String {
    let (a, b): (Vec<char>, Vec<char>) = (
        format!("{a:?}").chars().collect(),
        format!("{b:?}").chars().collect(),
    );
    let at = a
        .iter()
        .zip(&b)
        .position(|(x, y)| x != y)
        .unwrap_or(a.len().min(b.len()));
    let window = |s: &[char]| -> String {
        s[at.saturating_sub(150).min(s.len())..(at + 150).min(s.len())]
            .iter()
            .collect()
    };
    format!("\n  {}\n  {}", window(&a), window(&b))
}

/// The model of `back` equals that of `d`: header (as R2018, save time and
/// handle seed aside), classes, every object, summary (save time aside) and
/// preview. `dropped` lists the objects the writer leaves out.
fn assert_same_model(d: &Drawing, back: &Drawing, dropped: &[Handle], what: &str) {
    let mut header = d.header.to_r2018();
    assert_eq!(back.header.tdupdate, SAVED, "{what}");
    header.tdupdate = SAVED;
    header.handseed = back.header.handseed;
    assert_eq!(header, back.header, "{what}: header");
    assert_eq!(d.classes.len(), back.classes.len(), "{what}: classes");
    for (a, b) in d.classes.iter().zip(&back.classes) {
        assert_eq!(a, b, "{what}: class");
    }
    // Objects read before R2013 are compared as R2018 gives them back.
    let convert = d.source_version().is_some_and(|v| v < Version::R2013);
    let mut differ = Vec::new();
    for (h, o) in d.objects() {
        if dropped.contains(&h) {
            assert!(back.get(h).is_none(), "{what}: {h} was not left out");
            continue;
        }
        match back.get(h) {
            Some(b) if !convert && normalized(b) == normalized(o) => {}
            Some(b) if convert && *b == as_r2018(d, h, o) => {}
            Some(b) => differ.push(format!("{h}: {}", debug_diff(&as_r2018(d, h, o), b))),
            None => differ.push(format!("{h} missing")),
        }
    }
    assert!(
        differ.is_empty(),
        "{what}: {}",
        differ[..differ.len().min(3)].join("\n")
    );
    assert_eq!(
        d.objects().count() - dropped.len(),
        back.objects().count(),
        "{what}"
    );
    match (&d.summary, &back.summary) {
        (Some(a), Some(b)) => {
            let mut a = a.clone();
            a.modified = SAVED;
            assert_eq!(&a, b, "{what}: summary");
        }
        (None, Some(_)) => {}
        (a, b) => panic!("{what}: summary {a:?} became {b:?}"),
    }
    assert_eq!(d.preview, back.preview, "{what}: preview");
    assert_eq!(back.app_info, Some(AppInfo::opendwg()), "{what}: app info");
    // ACIS data, inline or in the data storage, is kept.
    for (h, o) in d.objects() {
        if matches!(
            o,
            Object::Entity {
                entity: Entity::Acis(_),
                ..
            }
        ) {
            assert_eq!(d.acis_data(h), back.acis_data(h), "{what}: {h} ACIS data");
        }
    }
}

/// The unconvertible objects of each corpus file, by type name and count:
/// R2000 and R2004 raw objects, and R2000 ACIS SAT data (R2004-R2010 ACIS
/// entities convert). The pre-R2010 CELLSTYLEMAP the converted table style
/// absorbs is not among them.
fn expected_unconvertible(name: &str) -> BTreeMap<&'static str, usize> {
    let raw_r2000: &[(&str, usize)] = &[
        ("3DSOLID", 2),
        ("ACAD_EVALUATION_GRAPH", 3),
        ("ACAD_TABLE", 2),
        ("ACDBASSOCPERSSUBENTMANAGER", 1),
        ("ACDBDETAILVIEWSTYLE", 2),
        ("ACDBPERSSUBENTMANAGER", 1),
        ("ACDBSECTIONVIEWSTYLE", 2),
        ("ACDB_BLOCKREPRESENTATION_DATA", 2),
        ("ACDB_DYNAMICBLOCKPURGEPREVENTER_VERSION", 1),
        ("ACSH_BOX_CLASS", 1),
        ("ACSH_EXTRUSION_CLASS", 1),
        ("ACSH_HISTORY_CLASS", 2),
        ("BLOCKGRIPLOCATIONCOMPONENT", 6),
        ("BLOCKLINEARGRIP", 2),
        ("BLOCKLINEARPARAMETER", 1),
        ("BLOCKSCALEACTION", 1),
        ("BLOCKVISIBILITYGRIP", 1),
        ("BLOCKVISIBILITYPARAMETER", 1),
        ("DATATABLE", 2),
        ("DBCOLOR", 1),
        ("DIMASSOC", 3),
        ("MATERIAL", 11),
        ("MESH", 2),
        ("PDFDEFINITION", 1),
        ("PDFUNDERLAY", 1),
        ("REGION", 1),
        ("TABLECONTENT", 2),
        ("TABLEGEOMETRY", 2),
        ("VISUALSTYLE", 24),
    ];
    let mut list: BTreeMap<&str, usize> = match name {
        "acadsharp-sample-ac1015" | "acadsharp-sample-ac1018" => {
            raw_r2000.iter().copied().collect()
        }
        _ => BTreeMap::new(),
    };
    // The R2004 sample's SAB converts; the R2000 sample's SAT does not.
    if name == "acadsharp-sample-ac1018" {
        list.remove("3DSOLID");
        list.remove("REGION");
    }
    list
}

/// Every corpus file round-trips: R2013 and R2018 files as they are; the
/// others refuse to save with exactly their unconvertible objects (none for
/// R2007 and R2010), and save once those are removed. R2007 and R2010 files
/// then read back with the model they had (the absorbed CELLSTYLEMAP aside,
/// ACIS entities in the R2013+ form with their data kept); R2000 and R2004
/// files convert, and the converted drawing round-trips unchanged.
#[test]
fn roundtrip_every_corpus_file() {
    for (e, p) in common::fetched_files_or_skip() {
        let what = e.name.as_str();
        let mut d = Drawing::open(&p).unwrap();
        let errors: Vec<_> = d
            .validate()
            .into_iter()
            .filter(|x| x.severity == Severity::Error)
            .collect();
        assert!(errors.is_empty(), "{what}: {errors:?}");
        let unconvertible = d.unconvertible_objects();
        let mut by_type: BTreeMap<&str, usize> = BTreeMap::new();
        for (_, t) in &unconvertible {
            *by_type.entry(t.as_str()).or_default() += 1;
        }
        assert_eq!(by_type, expected_unconvertible(what), "{what}");
        let source = d.source_version().unwrap();
        if source >= Version::R2013 {
            assert!(unconvertible.is_empty(), "{what}");
            let back = roundtrip(&d, what);
            assert_same_model(&d, &back, &[], what);
            assert_eq!(back.data_storage, d.data_storage, "{what}");
            continue;
        }
        if !unconvertible.is_empty() {
            match d.to_bytes_at(SAVED) {
                Err(Error::UnconvertibleObjects(list)) => {
                    assert_eq!(list, unconvertible, "{what}")
                }
                other => panic!("{what}: {:?}", other.map(|b| b.len())),
            }
        }
        remove_all(&mut d, &unconvertible, what);
        assert!(d.unconvertible_objects().is_empty(), "{what}");
        let back = roundtrip(&d, what);
        if source >= Version::R2007 {
            // The CELLSTYLEMAP the converted table style absorbed is left out.
            let dropped: Vec<Handle> = d
                .objects()
                .filter(|(h, o)| o.type_name() == "CELLSTYLEMAP" && back.get(*h).is_none())
                .map(|(h, _)| h)
                .collect();
            assert!(dropped.len() <= 1, "{what}: {dropped:?}");
            // Class counts drop by the objects removed or left out.
            let original = Drawing::open(&p).unwrap();
            let of_type = |d: &Drawing, name: &str| {
                d.objects().filter(|(_, o)| o.type_name() == name).count() as u32
            };
            let mut expected = d.clone();
            for c in &mut expected.classes {
                let gone = of_type(&original, &c.dxf_name) - of_type(&back, &c.dxf_name);
                c.instance_count = c.instance_count.saturating_sub(gone);
            }
            for &h in &dropped {
                let owner = d.owner_of(h).unwrap();
                if let Some(Object::Other {
                    object: NonGraphical::Dictionary(dict),
                    ..
                }) = expected.get_mut(owner)
                {
                    dict.entries.retain(|&(_, e)| e != h);
                }
            }
            assert_same_model(&expected, &back, &dropped, what);
        } else {
            // Converted: written again, it reads back unchanged.
            let again = roundtrip(&back, what);
            assert_same_model(&back, &again, &[], what);
        }
    }
}

/// From R2007 raw objects are copied bit for bit: every raw object's data,
/// strings and handles read back equal.
#[test]
fn roundtrip_raw_bytes_identical() {
    for (e, p) in common::fetched_files_or_skip() {
        let what = e.name.as_str();
        let mut d = Drawing::open(&p).unwrap();
        if d.source_version().unwrap() < Version::R2007 {
            continue;
        }
        let unconvertible = d.unconvertible_objects();
        remove_all(&mut d, &unconvertible, what);
        let back = roundtrip(&d, what);
        let mut raw = 0;
        for (h, o) in d.objects() {
            let (Some(r), Some(b)) = (o.raw(), back.get(h)) else {
                continue;
            };
            let b = b
                .raw()
                .unwrap_or_else(|| panic!("{what}: {h} is no longer raw"));
            assert_eq!(
                (&r.data, r.data_bits, &r.strings, &r.handles),
                (&b.data, b.data_bits, &b.strings, &b.handles),
                "{what}: {h}"
            );
            raw += 1;
        }
        assert!(raw > 0, "{what}: no raw objects");
    }
}

/// An R2018 drawing written back unchanged has every object byte for byte
/// as AutoCAD wrote it.
#[test]
fn r2018_objects_are_written_as_read() {
    for (e, p) in common::fetched_files_for("AC1032") {
        let source = std::fs::read(&p).unwrap();
        let d = Drawing::from_bytes(&source).unwrap();
        let bytes = d.to_bytes_at(SAVED).unwrap();
        let (a, b) = (common::object_bytes(&source), common::object_bytes(&bytes));
        assert_eq!(a.len(), b.len(), "{}", e.name);
        for (h, o) in &a {
            assert_eq!(Some(o), b.get(h), "{}: {h}", e.name);
        }
    }
}

fn texts(d: &Drawing) -> BTreeMap<Handle, String> {
    d.objects()
        .filter_map(|(h, o)| match o {
            Object::Entity {
                entity: Entity::Text(t),
                ..
            } => Some((h, t.value.clone())),
            Object::Entity {
                entity: Entity::MText(m),
                ..
            } => Some((h, m.contents.clone())),
            _ => None,
        })
        .collect()
}

/// Review Focus 2: TEXT and MTEXT strings survive a round trip exactly:
/// the corpus files tagged `non_ascii`, and a drawing given a surrogate
/// pair, an MTEXT `\U+` escape, CJK text and a layer named in Japanese.
#[test]
fn roundtrip_preserves_non_ascii() {
    for (e, p) in common::fetched_files_or_skip() {
        if !e.features.iter().any(|f| f == "non_ascii") {
            continue;
        }
        let mut d = Drawing::open(&p).unwrap();
        let unconvertible = d.unconvertible_objects();
        remove_all(&mut d, &unconvertible, &e.name);
        let before = texts(&d);
        assert!(
            before.values().any(|t| !t.is_ascii()),
            "{}: no non-ASCII text",
            e.name
        );
        assert_eq!(texts(&roundtrip(&d, &e.name)), before, "{}", e.name);
    }

    let Some(mut d) = open(R2018) else { return };
    const VALUE: &str = "𝄞 \\U+00E9 漢字";
    const LAYER: &str = "図面レイヤー";
    let layer0 = d.layer_by_name("0").unwrap();
    let Some(Object::Other {
        object: NonGraphical::Layer(l),
        ..
    }) = d.get(layer0)
    else {
        panic!("layer 0")
    };
    let mut layer = l.clone();
    layer.entry.name = LAYER.into();
    let layer = d.add_layer(layer).unwrap();
    let ms = d.model_space();
    let mut added = Vec::new();
    for want in ["TEXT", "MTEXT"] {
        let (common, entity) = d
            .objects()
            .find_map(|(_, o)| match o {
                Object::Entity { common, entity } if o.type_name() == want => {
                    Some((common.clone(), entity.clone()))
                }
                _ => None,
            })
            .unwrap();
        let mut common = common;
        common.obj.xdict = None;
        common.obj.reactors.clear();
        common.obj.xdata.clear();
        common.layer = layer;
        let entity = match entity {
            Entity::Text(mut t) => {
                t.value = VALUE.into();
                Entity::Text(t)
            }
            Entity::MText(mut m) => {
                m.contents = VALUE.into();
                Entity::MText(m)
            }
            _ => unreachable!(),
        };
        added.push(d.add_entity(ms, common, entity).unwrap());
    }
    let back = roundtrip(&d, "programmatic non-ASCII");
    assert_eq!(back.layer_by_name(LAYER), Some(layer));
    let t = texts(&back);
    for h in added {
        assert_eq!(t[&h], VALUE);
    }
    assert_eq!(texts(&back), texts(&d));
}

/// Review Focus 5: writing and reading are near-linear. 200k lines added to
/// a new drawing ([`Drawing::new_at`]) round-trip in under 10 s (release),
/// and the cost per added line from 20k to 200k lines is within 2x of the
/// cost per line of the first 20k (each time the median of three runs). The
/// deterministic companion is `drawing::write::tests::write_work_is_linear`.
#[test]
#[ignore = "large; run in release (Task 27 acceptance)"]
fn large_drawing_roundtrip_is_linear() {
    let base = Drawing::new_at(SAVED);
    let ms = base.model_space();
    let layer = base.layer_by_name("0").unwrap();
    let with_lines = |n: usize| {
        let mut d = base.clone();
        for i in 0..n {
            let c = EntityCommon::new(layer);
            let x = i as f64;
            let line = Entity::Line(Line {
                start: [x, 0.0, 0.0],
                end: [x, 1.0, 0.0],
                thickness: 0.0,
                extrusion: [0.0, 0.0, 1.0],
            });
            d.add_entity(ms, c, line).unwrap();
        }
        d
    };
    // The median of three round trips, in seconds.
    let time = |d: &Drawing| {
        let mut runs: Vec<f64> = (0..3)
            .map(|_| {
                let start = Instant::now();
                let bytes = d.to_bytes_at(SAVED).unwrap();
                let back = Drawing::from_bytes(&bytes).unwrap();
                let elapsed = start.elapsed().as_secs_f64();
                assert_eq!(back.objects().count(), d.objects().count());
                assert!(back.diagnostics().is_empty());
                elapsed
            })
            .collect();
        runs.sort_by(f64::total_cmp);
        runs[1]
    };
    let t0 = time(&base);
    let t20 = time(&with_lines(20_000));
    let t200 = time(&with_lines(200_000));
    let first = (t20 - t0) / 20_000.0;
    let rest = (t200 - t20) / 180_000.0;
    eprintln!(
        "new drawing {t0:.3} s, 20k lines {t20:.3} s, 200k lines {t200:.3} s; per line: first \
         20k {:.2} us, next 180k {:.2} us",
        first * 1e6,
        rest * 1e6
    );
    assert!(t200 < 10.0, "200k lines took {t200} s");
    assert!(first > 0.0, "20k lines took no longer than the new drawing");
    assert!(
        rest <= 2.0 * first,
        "per line: {rest} s past 20k against {first} s before: not linear"
    );
}

/// The same drawing and save time give the same bytes, from the drawing or a
/// copy of it.
#[test]
fn to_bytes_is_deterministic() {
    let Some(d) = open(R2018) else { return };
    let a = d.to_bytes_at(SAVED).unwrap();
    assert_eq!(a, d.to_bytes_at(SAVED).unwrap());
    assert_eq!(a, d.clone().to_bytes_at(SAVED).unwrap());
    // Another time changes the bytes; the drawing is not changed.
    let other = JulianDate {
        day: SAVED.day + 1,
        ..SAVED
    };
    assert_ne!(a, d.to_bytes_at(other).unwrap());
    assert_ne!(d.header.tdupdate, SAVED);
}

/// `save` writes the file through a temporary file in the same directory,
/// leaving none behind; a directory that does not exist is an error.
#[test]
fn save_writes_the_file_atomically() {
    let Some(d) = open(R2018) else { return };
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("save_atomic");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("out.dwg");
    std::fs::write(&path, b"old content").unwrap();
    d.save(&path).unwrap();
    let back = Drawing::open(&path).unwrap();
    assert!(back.diagnostics().is_empty());
    assert_eq!(back.objects().count(), d.objects().count());
    let names: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names, ["out.dwg"]);
    assert!(matches!(
        d.save(dir.join("missing").join("out.dwg")),
        Err(Error::Io(_))
    ));
}

/// `save` keeps an existing file's permissions, and writes through a
/// symbolic link, keeping the link.
#[cfg(unix)]
#[test]
fn save_keeps_permissions_and_links() {
    use std::os::unix::fs::PermissionsExt;
    let Some(d) = open(R2018) else { return };
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("save_perms");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let target = dir.join("target.dwg");
    std::fs::write(&target, b"old").unwrap();
    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o640)).unwrap();
    let link = dir.join("link.dwg");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    d.save(&link).unwrap();
    assert!(
        std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    let meta = std::fs::metadata(&target).unwrap();
    assert_eq!(meta.permissions().mode() & 0o777, 0o640);
    assert!(Drawing::open(&target).unwrap().diagnostics().is_empty());
    let mut names: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    names.sort();
    assert_eq!(names, ["link.dwg", "target.dwg"]);
    // A link whose target does not exist: an I/O error, nothing written.
    let dangling = dir.join("dangling.dwg");
    std::os::unix::fs::symlink(dir.join("missing.dwg"), &dangling).unwrap();
    assert!(matches!(d.save(&dangling), Err(Error::Io(_))));
    assert!(!dir.join("missing.dwg").exists());
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 3);
}

/// The save time and handle seed: $TDUPDATE is the save time, the summary's
/// modification date and the AuxHeader follow it, and $HANDSEED is past
/// every handle even when the header's was not.
#[test]
fn header_dates_and_seed_are_updated() {
    let Some(mut d) = open(R2018) else { return };
    let last = d.objects().last().unwrap().0;
    d.header.handseed = Handle(1);
    let bytes = d.to_bytes_at(SAVED).unwrap();
    let back = Drawing::from_bytes(&bytes).unwrap();
    assert_eq!(back.header.tdupdate, SAVED);
    assert_eq!(back.header.tdcreate, d.header.tdcreate);
    assert_eq!(back.summary.as_ref().unwrap().modified, SAVED);
    assert_eq!(back.header.handseed, Handle(last.0 + 1));
    let aux = back
        .extra_sections()
        .find(|(n, _)| **n == SectionName::AuxHeader)
        .unwrap()
        .1;
    assert_eq!(aux.len(), 129);
    let le = |at: usize| i32::from_le_bytes(aux[at..at + 4].try_into().unwrap());
    assert_eq!((le(77), le(81)), (SAVED.day, SAVED.ms));
    assert_eq!(le(85) as u64, last.0 + 1);
}

/// AppInfo is always opendwg's: AutoCAD's text, "Trusted DWG" included, is
/// never written back.
#[test]
fn app_info_is_opendwgs() {
    let Some(d) = open(R2018) else { return };
    assert!(
        d.app_info
            .as_ref()
            .unwrap()
            .entries
            .iter()
            .any(|e| e.text.contains("AutoCAD"))
    );
    let back = roundtrip(&d, R2018);
    assert_eq!(back.app_info, Some(AppInfo::opendwg()));
    let bytes = d.to_bytes_at(SAVED).unwrap();
    let raw = container::read(&bytes).unwrap();
    let app = &raw.sections[&SectionName::AppInfo];
    let text: Vec<u16> = app
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    assert!(!String::from_utf16_lossy(&text).contains("Trusted"));
}

/// The file header: maintenance 0; an R2018 source's undocumented fields
/// kept, a converted drawing's AutoCAD 2018 defaults.
#[test]
fn file_header_fields() {
    if let Some(p) = common::corpus_file_or_skip(R2018) {
        let source = std::fs::read(&p).unwrap();
        let bytes = Drawing::from_bytes(&source)
            .unwrap()
            .to_bytes_at(SAVED)
            .unwrap();
        assert_eq!(bytes[0x0B], 0);
        assert_eq!(bytes[0x34..0x48], source[0x34..0x48]);
    }
    if let Some(mut d) = open("acadsharp-sample-ac1024") {
        let unconvertible = d.unconvertible_objects();
        remove_all(&mut d, &unconvertible, "ac1024");
        let bytes = d.to_bytes_at(SAVED).unwrap();
        assert_eq!(&bytes[..6], b"AC1032");
        assert_eq!(bytes[0x0B], 0);
        let le = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
        assert_eq!(
            [le(0x34), le(0x38), le(0x3C), le(0x40)],
            [0x202, 0x17B, 0x149, 0x149]
        );
    }
}

/// A pre-R2010 CELLSTYLEMAP is absorbed by its converted table style: left
/// out with its dictionary entry, as AutoCAD saves R2010+. Its class stays.
#[test]
fn absorbed_cell_style_map_is_left_out() {
    let Some(mut d) = open("acadsharp-sample-ac1021") else {
        return;
    };
    let map = Handle(0xEFB);
    let dict = Handle(0x104);
    assert_eq!(d.get(map).unwrap().type_name(), "CELLSTYLEMAP");
    let unconvertible = d.unconvertible_objects();
    remove_all(&mut d, &unconvertible, "ac1021");
    let back = roundtrip(&d, "ac1021");
    assert!(back.get(map).is_none());
    let Some(Object::Other {
        object: NonGraphical::Dictionary(entries),
        ..
    }) = back.get(dict)
    else {
        panic!("dictionary 104")
    };
    assert!(entries.entries.is_empty(), "{:?}", entries.entries);
    assert!(back.classes.iter().any(|c| c.dxf_name == "CELLSTYLEMAP"));
    assert!(
        back.objects()
            .all(|(_, o)| !o.object_common().reactors.contains(&map))
    );
}

/// R2000's VP_ENT_HDR table is left out (R2004+ files have none), and an
/// R2000 drawing's other sections are regenerated in the R2018 form.
#[test]
fn r2000_conversion() {
    let Some(mut d) = open("acadsharp-sample-ac1015") else {
        return;
    };
    let headers: Vec<Handle> = d
        .objects()
        .filter(|(_, o)| matches!(o.type_name().as_ref(), "VP_ENT_HDR" | "VP_ENT_HDR_CONTROL"))
        .map(|(h, _)| h)
        .collect();
    assert_eq!(headers.len(), 4);
    let unconvertible = d.unconvertible_objects();
    remove_all(&mut d, &unconvertible, "ac1015");
    assert!(
        d.write_diagnostics().is_empty(),
        "{:?}",
        d.write_diagnostics()
    );
    let back = roundtrip(&d, "ac1015");
    for h in headers {
        assert!(back.get(h).is_none(), "{h}");
    }
    let sections: Vec<&SectionName> = back.extra_sections().map(|(n, _)| n).collect();
    for want in [
        SectionName::AuxHeader,
        SectionName::ObjFreeSpace,
        SectionName::Template,
        SectionName::RevHistory,
    ] {
        assert!(sections.contains(&&want), "{want:?} missing: {sections:?}");
    }
    assert!(back.summary.is_some());
    assert_eq!(back.preview, d.preview);
    // Layouts keep the viewports the read filled in.
    for (h, l) in d.layouts() {
        let Some(Object::Other {
            object: NonGraphical::Layout(b),
            ..
        }) = back.get(h)
        else {
            panic!("{h}")
        };
        assert_eq!(b.viewports, l.viewports);
    }
}

fn block_inserts(d: &Drawing, block: Handle) -> Vec<Handle> {
    match d.get(block) {
        Some(Object::Other {
            object: NonGraphical::BlockRecord(b),
            ..
        }) => b.inserts.clone(),
        o => panic!("{block}: {o:?}"),
    }
}

/// A block record's insert list is refreshed from the inserts that use the
/// block: an INSERT moved to another block through `get_mut` leaves the
/// first list and joins the second; lists whose users did not change keep
/// AutoCAD's entries, those of erased inserts included.
#[test]
fn block_insert_lists_are_refreshed() {
    let Some(mut d) = open(R2018) else { return };
    let (a, b) = (Handle(0x6F2), Handle(0x77F)); // MyBlock, my_block_v2
    let insert = Handle(0x704);
    assert_eq!(block_inserts(&d, a), [insert]);
    let b_before = block_inserts(&d, b);
    let Some(Object::Entity {
        entity: Entity::Insert(i),
        ..
    }) = d.get_mut(insert)
    else {
        panic!("insert")
    };
    i.block = b;
    let back = roundtrip(&d, "moved insert");
    assert!(block_inserts(&back, a).is_empty());
    let mut want = b_before;
    want.push(insert);
    assert_eq!(block_inserts(&back, b), want);
    // Unchanged lists are as read, stale entries included.
    let my_block = Handle(0xA0B);
    assert_eq!(block_inserts(&back, my_block), block_inserts(&d, my_block));
    assert!(
        block_inserts(&d, my_block)
            .iter()
            .any(|h| d.get(*h).is_none())
    );
}

/// A copy of viewport `vp` of `d`, as new common data and entity.
fn viewport_copy(d: &Drawing, vp: Handle) -> (EntityCommon, Entity) {
    let Some(Object::Entity { common, entity }) = d.get(vp).cloned() else {
        panic!("viewport")
    };
    let mut common = common;
    common.obj.xdict = None;
    common.obj.reactors.clear();
    common.obj.xdata.clear();
    (common, entity)
}

fn layout_viewports(d: &Drawing, layout: Handle) -> Vec<Handle> {
    match d.get(layout) {
        Some(Object::Other {
            object: NonGraphical::Layout(l),
            ..
        }) => l.viewports.clone(),
        o => panic!("{layout}: {o:?}"),
    }
}

/// A layout's viewport list is refreshed from its block's VIEWPORTs when
/// those changed since read: a new one is added, a removed one leaves, and
/// an entry owned by another block stays. A viewport taken out of the
/// block's entity list but still owned by the block is refused: its owner
/// no longer lists it (Task 23's validation).
#[test]
fn layout_viewports_are_refreshed() {
    let Some(base) = open(R2018) else { return };
    let (layout, l) = base
        .layouts()
        .into_iter()
        .find(|(_, l)| l.viewports.len() >= 2)
        .map(|(h, l)| (h, l.clone()))
        .unwrap();

    // Added.
    let mut d = base.clone();
    let (common, entity) = viewport_copy(&d, l.viewports[0]);
    let added = d.add_entity(l.block_record, common, entity).unwrap();
    let back = roundtrip(&d, "added viewport");
    let mut want = l.viewports.clone();
    want.push(added);
    assert_eq!(layout_viewports(&back, layout), want);

    // Taken out of the block's entity list: it leaves the layout's list.
    let mut d = base.clone();
    let gone = *l.viewports.last().unwrap();
    let Some(Object::Other {
        object: NonGraphical::BlockRecord(b),
        ..
    }) = d.get_mut(l.block_record)
    else {
        panic!("block")
    };
    let EntityList::Owned(list) = &mut b.entities else {
        panic!("list")
    };
    list.retain(|&e| e != gone);
    match d.to_bytes_at(SAVED) {
        Err(Error::Invalid(diags)) => {
            assert_eq!(diags.len(), 1, "{diags:?}");
            assert_eq!(diags[0].handle, Some(gone));
            assert!(diags[0].message.contains("does not list it"), "{diags:?}");
        }
        other => panic!("{:?}", other.map(|b| b.len())),
    }
    // Removed: it leaves the layout's list.
    let mut removed = base.clone();
    removed.remove(gone).unwrap();
    let back = roundtrip(&removed, "viewport removed");
    let mut want = l.viewports.clone();
    want.retain(|&v| v != gone);
    assert_eq!(layout_viewports(&back, layout), want);
    assert!(back.get(gone).is_none());

    // An entry owned by another block stays when the list is refreshed.
    let mut d = base.clone();
    let other = d
        .paper_spaces()
        .into_iter()
        .find(|&b| b != l.block_record)
        .unwrap();
    let (common, entity) = viewport_copy(&d, l.viewports[0]);
    let foreign = d.add_entity(other, common, entity).unwrap();
    if let Some(Object::Other {
        object: NonGraphical::Layout(l),
        ..
    }) = d.get_mut(layout)
    {
        l.viewports.push(foreign);
    }
    let (common, entity) = viewport_copy(&d, l.viewports[0]);
    let added = d.add_entity(l.block_record, common, entity).unwrap();
    let back = roundtrip(&d, "foreign viewport");
    let mut want = l.viewports.clone();
    want.extend([foreign, added]);
    assert_eq!(layout_viewports(&back, layout), want);
}

fn polyface(d: &Drawing) -> (Handle, i16, i16, Vec<Handle>) {
    d.objects()
        .find_map(|(h, o)| match o {
            Object::Entity {
                entity: Entity::PolyfaceMesh(p),
                ..
            } => match &p.vertices {
                EntityList::Owned(v) => Some((h, p.vertex_count, p.face_count, v.clone())),
                EntityList::Linked { .. } => None,
            },
            _ => None,
        })
        .unwrap()
}

/// The counts and vertex list of polyface mesh `h`.
fn polyface_counts(d: &Drawing, h: Handle) -> (i16, i16, usize) {
    match d.get(h) {
        Some(Object::Entity {
            entity: Entity::PolyfaceMesh(p),
            ..
        }) => match &p.vertices {
            EntityList::Owned(v) => (p.vertex_count, p.face_count, v.len()),
            l => panic!("{l:?}"),
        },
        o => panic!("{o:?}"),
    }
}

/// `common` without the links a new entity cannot have.
fn fresh(common: &EntityCommon) -> EntityCommon {
    let mut c = common.clone();
    c.obj.xdict = None;
    c.obj.reactors.clear();
    c.obj.xdata.clear();
    c
}

/// Mesh counts: a polyface mesh's counts are recounted whenever they
/// disagree with its vertices and faces (a changed list, or wrong counts);
/// a polygon mesh whose vertices no longer make M x N cannot be written.
#[test]
fn mesh_counts_are_refreshed() {
    let Some(base) = open(R2018) else { return };
    let (mesh, vertices, faces, list) = polyface(&base);
    assert_eq!((vertices, faces, list.len()), (5, 2, 7));

    // Unchanged list, wrong counts: recounted (the counts are derived).
    let mut d = base.clone();
    if let Some(Object::Entity {
        entity: Entity::PolyfaceMesh(p),
        ..
    }) = d.get_mut(mesh)
    {
        p.face_count = 9;
    }
    let back = roundtrip(&d, "counts recounted");
    assert_eq!(polyface_counts(&back, mesh), (5, 2, 7));

    // A face moved to another mesh (list and owner): both recounted.
    let mut d = base.clone();
    let ms = d.model_space();
    let Some(Object::Entity {
        common: mesh_common,
        entity: mesh_entity,
    }) = d.get(mesh).cloned()
    else {
        unreachable!()
    };
    let mut copy = mesh_entity;
    if let Some(v) = copy.polyline_vertices_mut() {
        *v = EntityList::Owned(vec![]);
    }
    let children = list
        .iter()
        .map(|&c| match d.get(c).cloned() {
            Some(Object::Entity { common, entity }) => (fresh(&common), entity),
            o => panic!("{o:?}"),
        })
        .collect();
    let other = d
        .add_polyline(ms, (fresh(&mesh_common), copy), children)
        .unwrap();
    let face = *list.last().unwrap();
    for (h, change) in [(mesh, false), (other, true)] {
        if let Some(Object::Entity { entity, .. }) = d.get_mut(h) {
            if let Some(EntityList::Owned(v)) = entity.polyline_vertices_mut() {
                if change {
                    v.push(face);
                } else {
                    v.pop();
                }
            }
        }
    }
    if let Some(Object::Entity { common, .. }) = d.get_mut(face) {
        common.obj.owner = other;
        common.entmode = 0;
    }
    let back = roundtrip(&d, "face moved");
    assert_eq!(polyface_counts(&back, mesh), (5, 1, 6));
    assert_eq!(polyface_counts(&back, other), (5, 3, 8));

    // A polygon mesh missing a vertex.
    let mut d = base.clone();
    let layer = d.layer_by_name("0").unwrap();
    let Some(Object::Entity { common, .. }) = d.get(mesh).cloned() else {
        unreachable!()
    };
    let mut common = common;
    common.obj.xdict = None;
    common.obj.reactors.clear();
    common.obj.xdata.clear();
    common.layer = layer;
    let vertex = |x: f64| {
        (
            common.clone(),
            Entity::VertexMesh(Vertex3d {
                flags: 64,
                point: [x, 0.0, 0.0],
            }),
        )
    };
    let grid = Entity::PolygonMesh(PolygonMesh {
        flags: 16,
        curve_type: 0,
        m_count: 2,
        n_count: 2,
        m_density: 0,
        n_density: 0,
        vertices: EntityList::Owned(vec![]),
        seqend: Handle(0),
    });
    let ms = d.model_space();
    let grid = d
        .add_polyline(
            ms,
            (common.clone(), grid),
            (0..4).map(|i| vertex(f64::from(i))).collect(),
        )
        .unwrap();
    assert!(d.to_bytes_at(SAVED).is_ok());
    if let Some(Object::Entity {
        entity: Entity::PolygonMesh(p),
        ..
    }) = d.get_mut(grid)
    {
        if let EntityList::Owned(v) = &mut p.vertices {
            v.pop();
        }
    }
    match d.to_bytes_at(SAVED) {
        Err(Error::Invalid(diags)) => {
            // The vertex taken out is still owned by the mesh: its owner no
            // longer lists it.
            assert_eq!(diags.len(), 2, "{diags:?}");
            assert!(
                diags
                    .iter()
                    .any(|x| x.handle == Some(grid) && x.message.contains("2 x 2")),
                "{diags:?}"
            );
            assert!(
                diags.iter().any(|x| x.message.contains("does not list it")),
                "{diags:?}"
            );
        }
        other => panic!("{:?}", other.map(|b| b.len())),
    }
}

/// Class instance counts move with the objects of the class: an R2018
/// drawing written unchanged keeps AutoCAD's counts (which include
/// instances not stored as objects); removed objects lower their class's
/// count, and a class left without objects keeps its entry.
#[test]
fn class_counts_follow_the_objects() {
    let Some(mut d) = open(R2018) else { return };
    let count = |d: &Drawing, name: &str| {
        d.classes
            .iter()
            .find(|c| c.dxf_name == name)
            .map(|c| c.instance_count)
    };
    let images: Vec<Handle> = d
        .objects()
        .filter(|(_, o)| o.type_name() == "IMAGE")
        .map(|(h, _)| h)
        .collect();
    assert!(!images.is_empty());
    let before = count(&d, "IMAGE").unwrap();
    for h in &images {
        d.remove(*h).unwrap();
    }
    let back = roundtrip(&d, "images removed");
    assert_eq!(
        count(&back, "IMAGE"),
        Some(before.saturating_sub(images.len() as u32))
    );
    let seen: HashSet<&str> = back.classes.iter().map(|c| c.dxf_name.as_str()).collect();
    assert!(d.classes.iter().all(|c| seen.contains(c.dxf_name.as_str())));
}

/// The corpus files with unconvertible objects (Task 25; the corpus has no
/// R2000/R2004 custom-object file, so the R2000 and R2004 samples stand in:
/// their raw custom-class objects, SAT ACIS data and raw history objects).
const WITH_UNCONVERTIBLES: [&str; 2] = ["acadsharp-sample-ac1015", "acadsharp-sample-ac1018"];

/// Saving refuses unconvertible objects with
/// [`Error::UnconvertibleObjects`], listing each by handle and type name:
/// for a raw object of a class type, its class's DXF name.
fn assert_refused_with_class_names(d: &Drawing, what: &str) -> Vec<(Handle, String)> {
    let list = match d.to_bytes_at(SAVED) {
        Err(Error::UnconvertibleObjects(list)) => list,
        other => panic!("{what}: {:?}", other.map(|b| b.len())),
    };
    assert!(!list.is_empty(), "{what}");
    assert_eq!(list, d.unconvertible_objects(), "{what}");
    let mut classed = 0;
    for (h, name) in &list {
        let raw = d.get(*h).unwrap().raw();
        if let Some(r) = raw.filter(|r| r.class_name.is_some()) {
            let class = d
                .classes
                .iter()
                .find(|c| c.class_number == r.type_code)
                .unwrap_or_else(|| panic!("{what}: {h} has no class"));
            assert_eq!(name, &class.dxf_name, "{what}: {h}");
            classed += 1;
        }
    }
    assert!(classed > 0, "{what}: no custom-class object listed");
    list
}

#[test]
fn old_custom_object_blocks_save() {
    for name in WITH_UNCONVERTIBLES {
        let Some(d) = open(name) else { continue };
        assert_refused_with_class_names(&d, name);
    }
    // The hermetic case, which always runs, is
    // `drawing::write::tests::hermetic_custom_object_blocks_save_until_removed`.
}

/// Once the unconvertible objects are removed through the API (references
/// to them cleared, dictionary entries naming them dropped by `remove`),
/// the drawing saves and reads back valid.
#[test]
fn removing_unconvertibles_allows_save() {
    let drawings: Vec<(String, Drawing)> = WITH_UNCONVERTIBLES
        .iter()
        .filter_map(|&name| Some((name.to_string(), open(name)?)))
        .collect();
    for (what, mut d) in drawings {
        let list = d.unconvertible_objects();
        assert!(!list.is_empty(), "{what}");
        remove_all(&mut d, &list, &what);
        assert!(d.unconvertible_objects().is_empty(), "{what}");
        for (h, _) in &list {
            assert!(d.get(*h).is_none(), "{what}: {h}");
        }
        // No dictionary names a removed object.
        for (dh, o) in d.objects() {
            if let Object::Other {
                object: NonGraphical::Dictionary(dict),
                ..
            } = o
            {
                for (name, e) in &dict.entries {
                    assert!(
                        !list.iter().any(|(h, _)| h == e),
                        "{what}: {dh} still lists {name} {e}"
                    );
                }
            }
        }
        let back = roundtrip(&d, &what);
        let errors: Vec<_> = back
            .validate()
            .into_iter()
            .filter(|x| x.severity == Severity::Error)
            .collect();
        assert!(errors.is_empty(), "{what}: {errors:?}");
    }
}
